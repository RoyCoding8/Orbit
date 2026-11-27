import { createContext, useContext, useMemo, useState, type ReactNode } from "react";
import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { harnesses, type Client } from "./harnesses";
export type { Client } from "./harnesses";

type HarnessContextValue = {
  client: Client;
  label: string;
  profile: typeof harnesses[Client];
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  projectRoot: string | null;
  setProjectRoot: (next: string | null) => void;
};

type InternalHarnessContextValue = HarnessContextValue & {
  hasProvider: boolean;
};

const HarnessContext = createContext<InternalHarnessContextValue>({
  client: "claude-code",
  label: "Claude Code",
  profile: harnesses["claude-code"],
  invoke: nativeInvoke,
  projectRoot: null,
  setProjectRoot: () => {},
  hasProvider: false,
});

export function HarnessProvider({
  client,
  projectRoot: controlledProjectRoot,
  setProjectRoot: controlledSetProjectRoot,
  children,
}: {
  client: Client;
  projectRoot?: string | null;
  setProjectRoot?: (next: string | null) => void;
  children: ReactNode;
}) {
  const [localProjectRoot, setLocalProjectRoot] = useState<string | null>(null);
  const projectRoot = controlledSetProjectRoot ? (controlledProjectRoot ?? null) : localProjectRoot;
  const setProjectRoot = controlledSetProjectRoot ?? setLocalProjectRoot;
  const value = useMemo<InternalHarnessContextValue>(() => ({
    client,
    label: harnesses[client].label,
    profile: harnesses[client],
    invoke: <T,>(command: string, args?: Record<string, unknown>) => nativeInvoke<T>(command, { ...args, client }),
    projectRoot,
    setProjectRoot,
    hasProvider: true,
  }), [client, projectRoot, setProjectRoot]);
  return <HarnessContext.Provider value={value}>{children}</HarnessContext.Provider>;
}

export function useHarness(): HarnessContextValue {
  const ctx = useContext(HarnessContext);
  const [fallbackProjectRoot, setFallbackProjectRoot] = useState<string | null>(null);
  return useMemo<HarnessContextValue>(() => (
    ctx.hasProvider
      ? ctx
      : {
          client: ctx.client,
          label: ctx.label,
          profile: ctx.profile,
          invoke: ctx.invoke,
          projectRoot: fallbackProjectRoot,
          setProjectRoot: setFallbackProjectRoot,
        }
  ), [ctx, fallbackProjectRoot]);
}
