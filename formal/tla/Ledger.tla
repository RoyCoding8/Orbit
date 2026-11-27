------------------------------ MODULE Ledger ------------------------------
EXTENDS Integers

CONSTANT MaxAttempts
ASSUME MaxAttempts \in (1..3)

Attempts == 1..MaxAttempts
NoFile == -1
Contents == NoFile..2
Targets == 1..2
InitialFiles == [t \in Targets |-> IF t = 1 THEN 0 ELSE NoFile]

VARIABLES attempt, phase, rows, before, after, backups, files, destination, published,
          stable, observationRequests, visible

vars == <<attempt, phase, rows, before, after, backups, files, destination, published,
          stable, observationRequests, visible>>

Init ==
    /\ attempt = 0
    /\ phase = "idle"
    /\ rows = [a \in Attempts |-> "absent"]
    /\ before = [a \in Attempts |-> 0]
    /\ after = [a \in Attempts |-> 0]
    /\ backups = [a \in Attempts |-> NoFile]
    /\ files = InitialFiles
    /\ destination = [a \in Attempts |-> 1]
    /\ published = {}
    /\ stable = FALSE
    /\ observationRequests = {}
    /\ visible = {}

Applied(a) == rows[a] = "applied"

Resolvable(a) ==
    /\ rows[a] = "pending"
    /\ files[destination[a]] = after[a]
    /\ backups[a] = before[a]

Superseded(a) ==
    /\ rows[a] = "pending"
    /\ \E later \in Attempts :
        later > a /\ rows[later] # "absent" /\ destination[later] = destination[a]

Recoverable(a) == Resolvable(a) /\ ~Superseded(a)

Start(t, content) ==
    /\ phase = "idle"
    /\ attempt < MaxAttempts
    /\ content # files[t]
    /\ attempt' = attempt + 1
    /\ after' = [after EXCEPT ![attempt + 1] = content]
    /\ destination' = [destination EXCEPT ![attempt + 1] = t]
    /\ phase' = "started"
    /\ UNCHANGED <<rows, before, backups, files, published, stable,
                   observationRequests, visible>>

Reconcile(a) ==
    /\ phase = "started"
    /\ destination[a] = destination[attempt]
    /\ Recoverable(a)
    /\ rows' = [rows EXCEPT ![a] = "applied"]
    /\ UNCHANGED <<attempt, phase, before, after, backups, files, destination, published,
                   stable, observationRequests, visible>>

Reserve ==
    /\ phase = "started"
    /\ ~\E a \in Attempts : destination[a] = destination[attempt] /\ Recoverable(a)
    /\ phase' = "reserved"
    /\ rows' = [rows EXCEPT ![attempt] = "pending"]
    /\ before' = [before EXCEPT ![attempt] = files[destination[attempt]]]
    /\ UNCHANGED <<attempt, after, backups, files, destination, published, stable,
                   observationRequests, visible>>

SaveBackup ==
    /\ phase = "reserved"
    /\ phase' = "backedUp"
    /\ backups' = [backups EXCEPT ![attempt] = before[attempt]]
    /\ UNCHANGED <<attempt, rows, before, after, files, destination, published, stable,
                   observationRequests, visible>>

Publish ==
    /\ phase = "backedUp"
    /\ phase' = "published"
    /\ files' = [files EXCEPT ![destination[attempt]] = after[attempt]]
    /\ published' = published \cup {attempt}
    /\ UNCHANGED <<attempt, rows, before, after, backups, destination, stable,
                   observationRequests, visible>>

VerifyReadback ==
    /\ phase = "published"
    /\ after[attempt] # NoFile
    /\ files[destination[attempt]] = after[attempt]
    /\ phase' = "verified"
    /\ UNCHANGED <<attempt, rows, before, after, backups, files, destination, published, stable,
                   observationRequests, visible>>

Commit ==
    /\ phase = "verified" \/ (phase = "published" /\ after[attempt] = NoFile)
    /\ phase' = "idle"
    /\ rows' = [rows EXCEPT ![attempt] = "applied"]
    /\ UNCHANGED <<attempt, before, after, backups, files, destination, published, stable,
                   observationRequests, visible>>

Interrupt ==
    /\ phase # "idle"
    /\ ~stable
    /\ phase' = "idle"
    /\ UNCHANGED <<attempt, rows, before, after, backups, files, destination, published, stable,
                   observationRequests, visible>>

Stabilize ==
    /\ ~stable
    /\ stable' = TRUE
    /\ UNCHANGED <<attempt, phase, rows, before, after, backups, files, destination, published,
                   observationRequests, visible>>

RequestObservation(a) ==
    /\ a \in published \ observationRequests
    /\ observationRequests' = observationRequests \cup {a}
    /\ UNCHANGED <<attempt, phase, rows, before, after, backups, files, destination, published,
                   stable, visible>>

Observe(a) ==
    /\ a \in observationRequests \ visible
    /\ Applied(a) \/ Recoverable(a)
    /\ visible' = visible \cup {a}
    /\ UNCHANGED <<attempt, phase, rows, before, after, backups, files, destination, published,
                   stable, observationRequests>>

Wait == phase = "idle" /\ UNCHANGED vars

Next ==
    \/ \E t \in Targets, content \in Contents : Start(t, content)
    \/ Reserve \/ SaveBackup \/ Publish \/ VerifyReadback \/ Commit
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
    /\ before \in [Attempts -> Contents]
    /\ after \in [Attempts -> Contents]
    /\ backups \in [Attempts -> Contents]
    /\ files \in [Targets -> Contents]
    /\ destination \in [Attempts -> Targets]
    /\ published \subseteq Attempts
    /\ stable \in BOOLEAN
    /\ observationRequests \subseteq Attempts
    /\ visible \subseteq Attempts

PublicationRequiresReservationAndBackup ==
    \A a \in published : rows[a] # "absent" /\ backups[a] = before[a]

AppliedRequiresPublication ==
    \A a \in Attempts : Applied(a) => a \in published

PendingPublicationCanBeFound ==
    \A a \in published : ~Applied(a) => Recoverable(a)

ObservationIsSound ==
    visible \subseteq observationRequests \cap published

FilesMatchTheirLastPublication ==
    \A t \in Targets :
        LET writes == {a \in published : destination[a] = t}
        IN IF writes = {}
           THEN files[t] = InitialFiles[t]
           ELSE \E a \in writes : files[t] = after[a] /\ \A b \in writes : b <= a

StableAttemptApplies ==
    \A a \in Attempts :
        (stable /\ attempt = a /\ phase # "idle") ~> Applied(a)

StablePublicationResolvesOnDemand ==
    \A a \in Attempts :
        (stable /\ a \in published
         /\ (a \in observationRequests
             \/ (attempt > a /\ phase = "started" /\ destination[attempt] = destination[a])))
        ~> (a \in visible \/ Applied(a))

=============================================================================
