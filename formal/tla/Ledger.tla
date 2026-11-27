------------------------------ MODULE Ledger ------------------------------
EXTENDS Integers

CONSTANT MaxAttempts
ASSUME MaxAttempts \in (1..3)

Attempts == 1..MaxAttempts

VARIABLES attempt, phase, rows, before, backups, target, published,
          stable, observationRequests, visible

vars == <<attempt, phase, rows, before, backups, target, published,
          stable, observationRequests, visible>>

Init ==
    /\ attempt = 0
    /\ phase = "idle"
    /\ rows = [a \in Attempts |-> "absent"]
    /\ before = [a \in Attempts |-> 0]
    /\ backups = [a \in Attempts |-> -1]
    /\ target = 0
    /\ published = {}
    /\ stable = FALSE
    /\ observationRequests = {}
    /\ visible = {}

Applied(a) == rows[a] = "applied"

Resolvable(a) ==
    /\ rows[a] = "pending"
    /\ target = a
    /\ backups[a] = before[a]

Superseded(a) ==
    /\ rows[a] = "pending"
    /\ \E later \in Attempts : later > a /\ rows[later] # "absent"

Start ==
    /\ phase = "idle"
    /\ attempt < MaxAttempts
    /\ attempt' = attempt + 1
    /\ phase' = "started"
    /\ UNCHANGED <<rows, before, backups, target, published, stable,
                   observationRequests, visible>>

Reconcile(a) ==
    /\ phase = "started"
    /\ Resolvable(a)
    /\ rows' = [rows EXCEPT ![a] = "applied"]
    /\ UNCHANGED <<attempt, phase, before, backups, target, published,
                   stable, observationRequests, visible>>

Reserve ==
    /\ phase = "started"
    /\ ~\E a \in Attempts : Resolvable(a)
    /\ phase' = "reserved"
    /\ rows' = [rows EXCEPT ![attempt] = "pending"]
    /\ before' = [before EXCEPT ![attempt] = target]
    /\ UNCHANGED <<attempt, backups, target, published, stable,
                   observationRequests, visible>>

SaveBackup ==
    /\ phase = "reserved"
    /\ phase' = "backedUp"
    /\ backups' = [backups EXCEPT ![attempt] = before[attempt]]
    /\ UNCHANGED <<attempt, rows, before, target, published, stable,
                   observationRequests, visible>>

Publish ==
    /\ phase = "backedUp"
    /\ phase' = "published"
    /\ target' = attempt
    /\ published' = published \cup {attempt}
    /\ UNCHANGED <<attempt, rows, before, backups, stable, observationRequests, visible>>

VerifyReadback ==
    /\ phase = "published"
    /\ target = attempt
    /\ phase' = "verified"
    /\ UNCHANGED <<attempt, rows, before, backups, target, published, stable,
                   observationRequests, visible>>

Commit ==
    /\ phase = "verified"
    /\ phase' = "idle"
    /\ rows' = [rows EXCEPT ![attempt] = "applied"]
    /\ UNCHANGED <<attempt, before, backups, target, published, stable,
                   observationRequests, visible>>

Interrupt ==
    /\ phase # "idle"
    /\ ~stable
    /\ phase' = "idle"
    /\ UNCHANGED <<attempt, rows, before, backups, target, published, stable,
                   observationRequests, visible>>

Stabilize ==
    /\ ~stable
    /\ stable' = TRUE
    /\ UNCHANGED <<attempt, phase, rows, before, backups, target, published,
                   observationRequests, visible>>

RequestObservation(a) ==
    /\ a \in published \ observationRequests
    /\ observationRequests' = observationRequests \cup {a}
    /\ UNCHANGED <<attempt, phase, rows, before, backups, target, published,
                   stable, visible>>

Observe(a) ==
    /\ a \in observationRequests \ visible
    /\ Applied(a) \/ (Resolvable(a) /\ ~Superseded(a))
    /\ visible' = visible \cup {a}
    /\ UNCHANGED <<attempt, phase, rows, before, backups, target, published,
                   stable, observationRequests>>

Wait == phase = "idle" /\ UNCHANGED vars

Next ==
    \/ Start \/ Reserve \/ SaveBackup \/ Publish \/ VerifyReadback \/ Commit
    \/ Interrupt \/ Stabilize \/ Wait
    \/ \E a \in Attempts : Reconcile(a) \/ RequestObservation(a) \/ Observe(a)

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(Reserve)
    /\ WF_vars(SaveBackup)
    /\ WF_vars(Publish)
    /\ WF_vars(VerifyReadback)
    /\ WF_vars(Commit)
    /\ \A a \in Attempts : WF_vars(Reconcile(a)) /\ WF_vars(Observe(a))

TypeOK ==
    /\ attempt \in 0..MaxAttempts
    /\ phase \in {"idle", "started", "reserved", "backedUp", "published", "verified"}
    /\ rows \in [Attempts -> {"absent", "pending", "applied"}]
    /\ before \in [Attempts -> 0..MaxAttempts]
    /\ backups \in [Attempts -> (-1)..MaxAttempts]
    /\ target \in 0..MaxAttempts
    /\ published \subseteq Attempts
    /\ stable \in BOOLEAN
    /\ observationRequests \subseteq Attempts
    /\ visible \subseteq Attempts

PublicationRequiresReservationAndBackup ==
    \A a \in published : rows[a] # "absent" /\ backups[a] = before[a]

AppliedRequiresPublication ==
    \A a \in Attempts : Applied(a) => a \in published

PendingPublicationCanBeFound ==
    \A a \in published : ~Applied(a) => Resolvable(a) /\ ~Superseded(a)

ObservationIsSound ==
    visible \subseteq observationRequests \cap published

StableAttemptApplies ==
    \A a \in Attempts :
        (stable /\ attempt = a /\ phase # "idle") ~> Applied(a)

StablePublicationResolvesOnDemand ==
    \A a \in Attempts :
        (stable /\ a \in published
         /\ (a \in observationRequests \/ (attempt > a /\ phase = "started")))
        ~> (a \in visible \/ Applied(a))

=============================================================================
