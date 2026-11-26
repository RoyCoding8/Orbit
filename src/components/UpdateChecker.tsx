import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { M3eButton } from "@m3e/react/button";
import { useModalDialog } from "../lib/useModalDialog";
import "../styles/UpdateChecker.css";

type State =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "error"; message: string }
  | { kind: "available"; update: Update; installing: boolean; installError: string };

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function UpdateChecker({ autoCheck = isTauriRuntime() }: { autoCheck?: boolean }) {
  const [state, setState] = useState<State>({ kind: "idle" });
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const runCheck = useCallback(async (silent: boolean) => {
    if (!silent) setState({ kind: "checking" });
    try {
      const update = await check();
      if (!mounted.current) {
        void update?.close?.().catch(() => {});
        return;
      }
      if (update) {
        setState({ kind: "available", update, installing: false, installError: "" });
      } else if (!silent) {
        setState({ kind: "up-to-date" });
      }
    } catch (error) {
      if (!mounted.current || silent) return;
      setState({ kind: "error", message: `Update check failed. ${errorMessage(error)}` });
    }
  }, []);

  useEffect(() => {
    if (autoCheck) void runCheck(true);
  }, [autoCheck, runCheck]);

  const dismiss = () => {
    if (state.kind !== "available" || state.installing) return;
    void state.update.close?.().catch(() => {});
    setState({ kind: "idle" });
  };

  const install = async () => {
    if (state.kind !== "available" || state.installing) return;
    const { update } = state;
    setState({ kind: "available", update, installing: true, installError: "" });
    try {
      await update.downloadAndInstall();
      await relaunch();
    } catch (error) {
      if (!mounted.current) return;
      setState({
        kind: "available",
        update,
        installing: false,
        installError: `Could not install update. ${errorMessage(error)}`,
      });
    }
  };

  const busy = state.kind === "checking" || (state.kind === "available" && state.installing);

  return (
    <div className="update-checker">
      <M3eButton type="button" variant="text" disabled={busy} onClick={() => void runCheck(false)}>
        {state.kind === "checking" ? "Checking…" : "Check for updates"}
      </M3eButton>
      {state.kind === "up-to-date" && (
        <span className="update-status" role="status">Up to date</span>
      )}
      {state.kind === "error" && (
        <span className="update-error" role="alert">{state.message}</span>
      )}
      {state.kind === "available" && (
        <UpdateDialog
          update={state.update}
          installing={state.installing}
          installError={state.installError}
          onDismiss={dismiss}
          onInstall={() => void install()}
        />
      )}
    </div>
  );
}

function UpdateDialog({
  update,
  installing,
  installError,
  onDismiss,
  onInstall,
}: {
  update: Update;
  installing: boolean;
  installError: string;
  onDismiss: () => void;
  onInstall: () => void;
}) {
  const dialog = useModalDialog(true);

  return (
    <dialog
      ref={dialog}
      className="edit-dialog update-dialog"
      aria-labelledby="update-dialog-title"
      onCancel={event => {
        event.preventDefault();
        if (!installing) onDismiss();
      }}
    >
      <div className="pane-heading">
        <div>
          <h3 id="update-dialog-title">Update available: v{update.version}</h3>
          {update.body && <p className="supporting-copy update-notes">{update.body}</p>}
        </div>
      </div>
      {installError && <p className="inline-error" role="alert">{installError}</p>}
      <div className="dialog-actions">
        <M3eButton type="button" variant="text" disabled={installing} onClick={onDismiss}>
          Later
        </M3eButton>
        <M3eButton type="button" variant="filled" disabled={installing} onClick={onInstall}>
          {installing ? "Installing…" : "Update & Restart"}
        </M3eButton>
      </div>
    </dialog>
  );
}
