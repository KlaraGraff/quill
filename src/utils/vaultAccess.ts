import { invoke, type InvokeArgs } from "@tauri-apps/api/core";

export type VaultAccessReason = "migrate";

export interface VaultAccessRequest {
  id: number;
  reason: VaultAccessReason;
  requestId?: string;
  flightKey: string;
  finish: (confirmed: boolean) => void;
}

let nextRequestId = 1;
let activeRequest: VaultAccessRequest | null = null;
const queue: VaultAccessRequest[] = [];
const subscribers = new Set<(request: VaultAccessRequest | null) => void>();
const authorizationAttempts = new Map<string, Promise<void>>();

function flightKeyFor(reason: VaultAccessReason, requestId?: string): string {
  return `${reason}:${requestId ?? "missing"}`;
}

function publish() {
  for (const subscriber of subscribers) subscriber(activeRequest);
}

function showNext() {
  if (activeRequest || queue.length === 0) return;
  activeRequest = queue.shift() ?? null;
  publish();
}

export function subscribeVaultAccess(
  subscriber: (request: VaultAccessRequest | null) => void,
): () => void {
  subscribers.add(subscriber);
  subscriber(activeRequest);
  showNext();
  return () => subscribers.delete(subscriber);
}

export function completeVaultAccess(id: number, confirmed: boolean) {
  if (activeRequest?.id !== id) return;
  const completed = activeRequest;
  const duplicates = queue.filter((request) => request.flightKey === completed.flightKey);
  for (let index = queue.length - 1; index >= 0; index -= 1) {
    if (queue[index].flightKey === completed.flightKey) queue.splice(index, 1);
  }
  activeRequest = null;
  publish();
  completed.finish(confirmed);
  for (const duplicate of duplicates) duplicate.finish(confirmed);
  showNext();
}

function confirmVaultAccess(
  reason: VaultAccessReason,
  requestId?: string,
): Promise<boolean> {
  return new Promise((finish) => {
    queue.push({
      id: nextRequestId++,
      reason,
      requestId,
      flightKey: flightKeyFor(reason, requestId),
      finish,
    });
    showNext();
  });
}

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function confirmationFromError(error: unknown): {
  reason: VaultAccessReason;
  requestId?: string;
} | null {
  const match = errorText(error).match(
    /VAULT_CONFIRM_REQUIRED:(migrate):([0-9a-f-]+)/i,
  );
  if (!match) return null;
  return {
    reason: match[1].toLowerCase() as VaultAccessReason,
    requestId: match[2],
  };
}

async function authorizeAfterConfirmation(
  reason: VaultAccessReason,
  requestId?: string,
): Promise<void> {
  // One explicit migration click owns one authorization flight. Routine AI
  // commands cannot enter this path because only the migration command emits
  // VAULT_CONFIRM_REQUIRED:migrate.
  const key = flightKeyFor(reason, requestId);
  const existing = authorizationAttempts.get(key);
  if (existing) return existing;

  const attempt = (async () => {
    const confirmed = await confirmVaultAccess(reason, requestId);
    if (!confirmed) {
      try {
        await invoke("vault_deny", { reason, requestId: requestId ?? null });
      } catch (error) {
        if (!/VAULT_MIGRATION_REQUEST_EXPIRED/i.test(errorText(error))) throw error;
      }
      throw new Error("VAULT_USER_CANCELLED");
    }
    try {
      await invoke("vault_authorize", { reason, requestId: requestId ?? null });
    } catch (error) {
      // Another webview may have completed the shared backend request first.
      // Re-check the original operation without presenting another prompt.
      if (/VAULT_MIGRATION_REQUEST_EXPIRED/i.test(errorText(error))) return;
      throw error;
    }
  })();
  authorizationAttempts.set(key, attempt);
  try {
    await attempt;
  } finally {
    authorizationAttempts.delete(key);
  }
}

export async function invokeWithCredentialMigration<T>(
  command: string,
  args?: InvokeArgs,
  shouldContinue?: () => boolean,
): Promise<T> {
  const assertShouldContinue = () => {
    if (shouldContinue && !shouldContinue()) {
      throw new Error("REQUEST_CANCELLED");
    }
  };

  assertShouldContinue();
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    const confirmation = confirmationFromError(error);
    if (!confirmation) throw error;
    await authorizeAfterConfirmation(confirmation.reason, confirmation.requestId);
  }

  // A migration view may close while the user is deciding. Do not retry a
  // stale action after authorization completes.
  assertShouldContinue();
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    // Never turn one click into a chain of authorization prompts. A fresh
    // confirmation requirement can be handled only by a later user action.
    if (confirmationFromError(error)) {
      throw new Error("VAULT_ACCESS_RETRY_REQUIRED");
    }
    throw error;
  }
}
