import { createContext, useContext, useMemo, type ReactNode } from "react";
import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { harnesses, type Client } from "./harnesses";
export type { Client } from "./harnesses";

type HarnessContextValue = {
  client: Client;
  label: string;
  profile: typeof harnesses[Client];
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
};

const HarnessContext = createContext<HarnessContextValue>({
  client: "claude-code",
  label: "Claude Code",
  profile: harnesses["claude-code"],
  invoke: nativeInvoke,
});

export function HarnessProvider({ client, children }: { client: Client; children: ReactNode }) {
  const value = useMemo<HarnessContextValue>(() => ({
    client,
    label: harnesses[client].label,
    profile: harnesses[client],
    invoke: <T,>(command: string, args?: Record<string, unknown>) => nativeInvoke<T>(command, { ...args, client }),
  }), [client]);
  return <HarnessContext.Provider value={value}>{children}</HarnessContext.Provider>;
}

export function useHarness() {
  return useContext(HarnessContext);
}
