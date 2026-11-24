/**
 * The room client token.
 *
 * Kept where the AI provider keys are kept — the OS keychain on the desktop,
 * the server's secret store on the web build — rather than in preferences,
 * which are a plain file. It is a credential for someone else's machine, and
 * one that survives being read: nothing rotates it for you.
 */

import { invoke } from "@tauri-apps/api/core";

const SERVICE = "phantom-room";
const ACCOUNT = "client-token";

export async function getRoomToken(): Promise<string | null> {
  try {
    const v = await invoke<string | null>("secrets_get", {
      service: SERVICE,
      account: ACCOUNT,
    });
    return v && v.length > 0 ? v : null;
  } catch {
    // No secret store on this backend; the room is simply unconfigured.
    return null;
  }
}

export async function setRoomToken(token: string): Promise<void> {
  const trimmed = token.trim();
  if (!trimmed) {
    await invoke("secrets_delete", { service: SERVICE, account: ACCOUNT });
    return;
  }
  await invoke("secrets_set", {
    service: SERVICE,
    account: ACCOUNT,
    password: trimmed,
  });
}
