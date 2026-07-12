export const AI_ERROR_CODES = [
  "AI_NOT_CONFIGURED",
  "AI_KEYS_DISABLED",
  "AI_ALL_KEYS_INVALID",
  "AI_KEYS_COOLING_DOWN",
  "AI_NO_USABLE_KEYS",
  "AI_STREAM_FAILED",
] as const;

export type AiErrorCode = (typeof AI_ERROR_CODES)[number];

const AI_SETTINGS_ERROR_CODES = new Set<AiErrorCode>([
  "AI_NOT_CONFIGURED",
  "AI_KEYS_DISABLED",
  "AI_ALL_KEYS_INVALID",
  "AI_KEYS_COOLING_DOWN",
  "AI_NO_USABLE_KEYS",
]);

export function getAiErrorCode(error: unknown): AiErrorCode | null {
  const message = String(error);
  return AI_ERROR_CODES.find((code) => message.includes(code)) ?? null;
}

export function isAiErrorCode(value: unknown): value is AiErrorCode {
  return typeof value === "string" && AI_ERROR_CODES.includes(value as AiErrorCode);
}

export function isAiSettingsError(code: AiErrorCode | null): boolean {
  return code !== null && AI_SETTINGS_ERROR_CODES.has(code);
}

export function aiErrorMessageKey(code: AiErrorCode): string {
  switch (code) {
    case "AI_NOT_CONFIGURED":
      return "ai.notConfigured";
    case "AI_KEYS_DISABLED":
      return "ai.keysDisabled";
    case "AI_ALL_KEYS_INVALID":
      return "ai.allKeysInvalid";
    case "AI_KEYS_COOLING_DOWN":
      return "ai.keysCoolingDown";
    case "AI_NO_USABLE_KEYS":
      return "ai.noUsableKeys";
    case "AI_STREAM_FAILED":
      return "ai.requestFailed";
  }
}
