import { useEffect, useRef, useState } from "react";
import { KeyRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  completeVaultAccess,
  subscribeVaultAccess,
  type VaultAccessRequest,
} from "../utils/vaultAccess";
import Button from "./ui/Button";

export default function VaultAccessDialog() {
  const { t } = useTranslation();
  const [request, setRequest] = useState<VaultAccessRequest | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => subscribeVaultAccess((next) => {
    setRequest(next);
  }), []);

  useEffect(() => {
    if (!request) return;
    cancelRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      completeVaultAccess(request.id, false);
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => document.removeEventListener("keydown", onKeyDown, true);
  }, [request]);

  if (!request) return null;

  const confirm = () => {
    // Close the explanation before macOS presents its own secure prompt.
    completeVaultAccess(request.id, true);
  };

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center bg-overlay px-4"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          completeVaultAccess(request.id, false);
        }
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="vault-access-title"
        className="w-[460px] max-w-full rounded-lg border border-border bg-bg-surface p-5 shadow-popover"
      >
        <div className="flex items-start gap-3">
          <div className="flex size-9 shrink-0 items-center justify-center rounded-md bg-accent-bg text-accent-text">
            <KeyRound size={18} />
          </div>
          <div className="min-w-0">
            <h2 id="vault-access-title" className="text-[16px] font-semibold text-text-primary">
              {t(`vaultAccess.${request.reason}.title`)}
            </h2>
            <p className="mt-1 text-[13px] leading-5 text-text-secondary">
              {t(`vaultAccess.${request.reason}.message`)}
            </p>
          </div>
        </div>

        <div className="mt-4 rounded-md border border-border-light bg-bg-input px-3 py-2.5 text-[12px] leading-5 text-text-muted">
          {t("vaultAccess.systemPromptHint")}
        </div>

        <div className="mt-5 flex justify-end gap-2">
          <Button
            ref={cancelRef}
            type="button"
            variant="ghost"
            onClick={() => completeVaultAccess(request.id, false)}
          >
            {t("common.cancel")}
          </Button>
          <Button type="button" onClick={confirm}>
            {t(`vaultAccess.${request.reason}.confirm`)}
          </Button>
        </div>
      </div>
    </div>
  );
}
