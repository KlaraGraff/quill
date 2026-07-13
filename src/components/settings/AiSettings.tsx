import { useCallback, useEffect, useMemo, useState, type DragEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, Loader2, Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "../ui/Button";
import AiServiceCard, {
  type AiConnectionTestResult,
  type AiCredential,
  type AiProfile,
} from "./AiServiceCard";
import type { SettingsProps } from "./types";

interface AiSettingsProps extends SettingsProps {
  onSaveRef?: (save: (() => void) | null) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

interface OAuthStatus {
  connected: boolean;
  account_id: string | null;
}

const PROFILE_CONFIG_KEYS = [
  "label",
  "provider",
  "auth_mode",
  "base_url",
  "model",
  "temperature",
  "keep_alive",
] as const;

function sameProfileConfig(left: AiProfile | undefined, right: AiProfile | undefined): boolean {
  if (!left || !right) return false;
  return PROFILE_CONFIG_KEYS.every((key) => left[key] === right[key]);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function profileLabel(value: string): string {
  return Array.from(value).slice(0, 100).join("");
}

function updateOne<T extends { id: string }>(items: T[], id: string, patch: Partial<T>): T[] {
  return items.map((item) => item.id === id ? { ...item, ...patch } : item);
}

export default function AiSettings({ showSavedToast, onSaveRef, onDirtyChange }: AiSettingsProps) {
  const { t } = useTranslation();
  const [profiles, setProfiles] = useState<AiProfile[]>([]);
  const [savedProfiles, setSavedProfiles] = useState<AiProfile[]>([]);
  const [credentials, setCredentials] = useState<Record<string, AiCredential[]>>({});
  const [modelOptions, setModelOptions] = useState<Record<string, string[]>>({});
  const [testResults, setTestResults] = useState<Record<string, AiConnectionTestResult>>({});
  const [staleHealthIds, setStaleHealthIds] = useState<Set<string>>(() => new Set());
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [modelsLoadingId, setModelsLoadingId] = useState<string | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [dropTargetId, setDropTargetId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [oauthStatus, setOauthStatus] = useState<OAuthStatus>({ connected: false, account_id: null });
  const [oauthLoading, setOauthLoading] = useState(false);

  const dirtyIds = useMemo(() => {
    const saved = new Map(savedProfiles.map((profile) => [profile.id, profile]));
    return new Set(profiles.filter((profile) => !sameProfileConfig(profile, saved.get(profile.id))).map((profile) => profile.id));
  }, [profiles, savedProfiles]);

  const refreshCredentials = useCallback(async (profileId: string) => {
    const next = await invoke<AiCredential[]>("ai_list_credentials", { profileId });
    setCredentials((current) => ({ ...current, [profileId]: next }));
    return next;
  }, []);

  const refreshOAuthStatus = useCallback(async () => {
    const next = await invoke<OAuthStatus>("openai_oauth_status");
    setOauthStatus(next);
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const nextProfiles = await invoke<AiProfile[]>("ai_list_profiles");
      const credentialEntries = await Promise.all(
        nextProfiles.map(async (profile) => [
          profile.id,
          await invoke<AiCredential[]>("ai_list_credentials", { profileId: profile.id }),
        ] as const),
      );
      setProfiles(nextProfiles);
      setSavedProfiles(nextProfiles);
      setCredentials(Object.fromEntries(credentialEntries));
      setExpandedId((current) => current && nextProfiles.some((profile) => profile.id === current) ? current : null);
      try {
        await refreshOAuthStatus();
      } catch {
        // OAuth is optional; profile and API-key configuration remain usable.
      }
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setLoading(false);
    }
  }, [refreshOAuthStatus]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    onDirtyChange?.(dirtyIds.size > 0);
  }, [dirtyIds, onDirtyChange]);

  const updateProfile = useCallback((id: string, patch: Partial<AiProfile>) => {
    setProfiles((current) => updateOne(current, id, patch));
    if (["provider", "auth_mode", "base_url", "model"].some((key) => key in patch)) {
      setStaleHealthIds((current) => new Set(current).add(id));
      setTestResults((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
    if (["provider", "auth_mode", "base_url"].some((key) => key in patch)) {
      setModelOptions((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
    setError(null);
  }, []);

  const markHealthStale = useCallback((profileId: string) => {
    setStaleHealthIds((current) => new Set(current).add(profileId));
    setTestResults((current) => {
      const next = { ...current };
      delete next[profileId];
      return next;
    });
  }, []);

  const persistProfile = useCallback(async (profile: AiProfile): Promise<AiProfile> => {
    const saved = await invoke<AiProfile>("ai_update_profile", {
      id: profile.id,
      label: profile.label,
      provider: profile.provider,
      authMode: profile.auth_mode,
      baseUrl: profile.base_url?.trim() || null,
      model: profile.model,
      temperature: profile.temperature,
      keepAlive: profile.keep_alive?.trim() || null,
    });
    setProfiles((current) => updateOne(current, saved.id, saved));
    setSavedProfiles((current) => updateOne(current, saved.id, saved));
    return saved;
  }, []);

  const saveProfiles = useCallback(async () => {
    const pending = profiles.filter((profile) => dirtyIds.has(profile.id));
    if (pending.length === 0) return;
    setSaving(true);
    setError(null);
    try {
      for (const profile of pending) await persistProfile(profile);
      showSavedToast(t("settings.ai.savedToast"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setSaving(false);
    }
  }, [dirtyIds, persistProfile, profiles, showSavedToast, t]);

  const requestSave = useCallback(() => {
    void saveProfiles();
  }, [saveProfiles]);

  useEffect(() => {
    onSaveRef?.(requestSave);
    return () => onSaveRef?.(null);
  }, [onSaveRef, requestSave]);

  const createProfile = async () => {
    setBusyId("new");
    setError(null);
    try {
      const created = await invoke<AiProfile>("ai_create_profile", {
        label: t("settings.ai.newServiceName"),
        provider: "openai",
        authMode: "api_key",
        baseUrl: "https://api.openai.com",
        model: "gpt-4o-mini",
        temperature: 0.3,
        keepAlive: null,
        enabled: true,
      });
      setProfiles((current) => [...current, created]);
      setSavedProfiles((current) => [...current, created]);
      setCredentials((current) => ({ ...current, [created.id]: [] }));
      setExpandedId(created.id);
      showSavedToast(t("settings.ai.serviceCreated"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  };

  const duplicateProfile = async (profile: AiProfile) => {
    setBusyId(profile.id);
    setError(null);
    let duplicateId: string | null = null;
    try {
      const duplicate = await invoke<AiProfile>("ai_duplicate_profile", {
        id: profile.id,
        label: profileLabel(t("settings.ai.copyServiceName", { name: profile.label })),
      });
      duplicateId = duplicate.id;
      const configured = await invoke<AiProfile>("ai_update_profile", {
        id: duplicate.id,
        label: duplicate.label,
        provider: profile.provider,
        authMode: profile.auth_mode,
        baseUrl: profile.base_url?.trim() || null,
        model: profile.model,
        temperature: profile.temperature,
        keepAlive: profile.keep_alive?.trim() || null,
      });
      setProfiles((current) => [...current, configured]);
      setSavedProfiles((current) => [...current, configured]);
      setCredentials((current) => ({ ...current, [configured.id]: [] }));
      setExpandedId(configured.id);
      showSavedToast(t("settings.ai.serviceDuplicated"));
    } catch (nextError) {
      // `duplicate` and `update` are separate commands. If applying unsaved
      // draft values fails, remove the just-created shell so it cannot reappear
      // as an unexpected extra service after the settings page is reopened.
      if (duplicateId) {
        try {
          await invoke("ai_delete_profile", { id: duplicateId });
        } catch {
          // Preserve the original, actionable update error for the user.
        }
      }
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  };

  const deleteProfile = async (id: string) => {
    setBusyId(id);
    setError(null);
    try {
      await invoke("ai_delete_profile", { id });
      setProfiles((current) => current.filter((profile) => profile.id !== id));
      setSavedProfiles((current) => current.filter((profile) => profile.id !== id));
      setCredentials((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
      setModelOptions((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
      setTestResults((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
      setStaleHealthIds((current) => {
        const next = new Set(current);
        next.delete(id);
        return next;
      });
      setExpandedId((current) => current === id ? null : current);
      showSavedToast(t("settings.ai.serviceDeleted"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  };

  const toggleProfile = async (id: string, enabled: boolean) => {
    const previous = profiles.find((profile) => profile.id === id)?.enabled ?? !enabled;
    setBusyId(id);
    setProfiles((current) => updateOne(current, id, { enabled }));
    setSavedProfiles((current) => updateOne(current, id, { enabled }));
    setError(null);
    try {
      await invoke("ai_set_profile_enabled", { id, enabled });
    } catch (nextError) {
      setProfiles((current) => updateOne(current, id, { enabled: previous }));
      setSavedProfiles((current) => updateOne(current, id, { enabled: previous }));
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  };

  const applyProfileOrder = useCallback(async (next: AiProfile[]) => {
    const previousProfiles = profiles;
    const previousSaved = savedProfiles;
    const withPriority = next.map((profile, priority) => ({ ...profile, priority }));
    const nextSaved = withPriority.map((profile) => {
      const saved = previousSaved.find((item) => item.id === profile.id);
      return saved ? { ...saved, priority: profile.priority } : profile;
    });
    setProfiles(withPriority);
    setSavedProfiles(nextSaved);
    setBusyId("order");
    setError(null);
    try {
      await invoke("ai_reorder_profiles", { ids: withPriority.map((profile) => profile.id) });
    } catch (nextError) {
      setProfiles(previousProfiles);
      setSavedProfiles(previousSaved);
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  }, [profiles, savedProfiles]);

  const moveProfile = async (id: string, direction: -1 | 1) => {
    const index = profiles.findIndex((profile) => profile.id === id);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= profiles.length) return;
    const next = [...profiles];
    [next[index], next[target]] = [next[target], next[index]];
    await applyProfileOrder(next);
  };

  const dropProfile = async (targetId: string) => {
    const sourceId = draggingId;
    setDraggingId(null);
    setDropTargetId(null);
    if (!sourceId || sourceId === targetId) return;
    const sourceIndex = profiles.findIndex((profile) => profile.id === sourceId);
    const targetIndex = profiles.findIndex((profile) => profile.id === targetId);
    if (sourceIndex < 0 || targetIndex < 0) return;
    const next = [...profiles];
    const [moved] = next.splice(sourceIndex, 1);
    next.splice(targetIndex, 0, moved);
    await applyProfileOrder(next);
  };

  const testProfile = async (profile: AiProfile) => {
    setTestingId(profile.id);
    setError(null);
    try {
      const persisted = dirtyIds.has(profile.id) ? await persistProfile(profile) : profile;
      const result = await invoke<AiConnectionTestResult>("ai_test_profile", { id: persisted.id });
      setTestResults((current) => ({ ...current, [profile.id]: result }));
      setStaleHealthIds((current) => {
        const next = new Set(current);
        next.delete(profile.id);
        return next;
      });
      setProfiles((current) => updateOne(current, profile.id, {
        state: result.success ? "active" : "cooldown",
        last_error_kind: result.error_kind ?? null,
        last_used_at: result.tested_at,
        last_latency_ms: result.total_ms,
      }));
      setSavedProfiles((current) => updateOne(current, profile.id, {
        state: result.success ? "active" : "cooldown",
        last_error_kind: result.error_kind ?? null,
        last_used_at: result.tested_at,
        last_latency_ms: result.total_ms,
      }));
      showSavedToast(result.success ? t("settings.ai.connectionAvailable") : t("settings.ai.connectionUnavailable"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setTestingId(null);
    }
  };

  const fetchModels = async (profile: AiProfile) => {
    setModelsLoadingId(profile.id);
    setError(null);
    try {
      const models = await invoke<string[]>("ai_list_models", {
        profileId: profile.id,
        provider: profile.provider,
        authMode: profile.auth_mode,
        baseUrl: profile.base_url?.trim() || null,
      });
      setModelOptions((current) => ({ ...current, [profile.id]: models }));
      showSavedToast(t("settings.ai.modelsLoaded", { count: models.length }));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setModelsLoadingId(null);
    }
  };

  const addCredential = async (profileId: string, label: string, value: string) => {
    setError(null);
    try {
      await invoke("ai_add_credential", { profileId, label, value });
      await refreshCredentials(profileId);
      markHealthStale(profileId);
      showSavedToast(t("settings.ai.keyAdded"));
    } catch (nextError) {
      setError(errorText(nextError));
      throw nextError;
    }
  };

  const replaceCredential = async (profileId: string, id: string, value: string) => {
    setError(null);
    try {
      await invoke("ai_replace_credential", { id, value });
      await refreshCredentials(profileId);
      markHealthStale(profileId);
      showSavedToast(t("settings.ai.keyReplaced"));
    } catch (nextError) {
      setError(errorText(nextError));
      throw nextError;
    }
  };

  const toggleCredential = async (profileId: string, id: string, enabled: boolean) => {
    setError(null);
    try {
      await invoke("ai_set_credential_enabled", { id, enabled });
      await refreshCredentials(profileId);
      markHealthStale(profileId);
    } catch (nextError) {
      setError(errorText(nextError));
      throw nextError;
    }
  };

  const deleteCredential = async (profileId: string, id: string) => {
    setError(null);
    try {
      await invoke("ai_delete_credential", { id });
      await refreshCredentials(profileId);
      markHealthStale(profileId);
    } catch (nextError) {
      setError(errorText(nextError));
      throw nextError;
    }
  };

  const reorderCredentials = async (profileId: string, ids: string[]) => {
    setError(null);
    try {
      await invoke("ai_reorder_credentials", { ids });
      await refreshCredentials(profileId);
      markHealthStale(profileId);
    } catch (nextError) {
      setError(errorText(nextError));
      throw nextError;
    }
  };

  const loginWithOpenAi = async (profile: AiProfile) => {
    setOauthLoading(true);
    setError(null);
    try {
      const oauthProfile = { ...profile, auth_mode: "oauth" as const, base_url: null };
      await persistProfile(oauthProfile);
      const status = await invoke<OAuthStatus>("openai_oauth_login");
      setOauthStatus(status);
      markHealthStale(profile.id);
      showSavedToast(t("settings.ai.oauthSuccess"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setOauthLoading(false);
    }
  };

  const logoutFromOpenAi = async () => {
    setOauthLoading(true);
    setError(null);
    try {
      await invoke("openai_oauth_logout");
      setOauthStatus({ connected: false, account_id: null });
      const affectedIds = profiles
        .filter((profile) => profile.provider === "openai" && profile.auth_mode === "oauth")
        .map((profile) => profile.id);
      setStaleHealthIds((current) => {
        const next = new Set(current);
        for (const id of affectedIds) next.add(id);
        return next;
      });
      setTestResults((current) => {
        const next = { ...current };
        for (const id of affectedIds) delete next[id];
        return next;
      });
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setOauthLoading(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center gap-2 py-8 text-[13px] text-text-muted">
        <Loader2 size={16} className="animate-spin" />
        {t("settings.ai.loadingServices")}
      </div>
    );
  }

  return (
    <div className="pb-6 pt-2">
      <div className="mb-3 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.services")}</h4>
          <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.servicesHint")}</p>
        </div>
        <Button variant="secondary" size="sm" onClick={() => void createProfile()} disabled={busyId != null || saving}>
          {busyId === "new" ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}
          {t("settings.ai.addService")}
        </Button>
      </div>

      {error && (
        <div role="alert" className="mb-3 flex items-start gap-2 rounded-md bg-danger-bg px-3 py-2 text-[11px] leading-5 text-danger-text">
          <AlertCircle size={14} className="mt-0.5 shrink-0" />
          <span className="min-w-0 break-words">{error}</span>
        </div>
      )}

      {profiles.length === 0 ? (
        <div className="flex min-h-32 flex-col items-center justify-center rounded-lg border border-dashed border-border px-4 text-center">
          <p className="text-[13px] font-medium text-text-primary">{t("settings.ai.noServices")}</p>
          <p className="mt-1 text-[11px] text-text-muted">{t("settings.ai.noServicesHint")}</p>
        </div>
      ) : (
        <div className="space-y-2">
          {profiles.map((profile, index) => (
            <AiServiceCard
              key={profile.id}
              profile={profile}
              credentials={credentials[profile.id] ?? []}
              expanded={expandedId === profile.id}
              dirty={dirtyIds.has(profile.id)}
              busy={saving || oauthLoading || busyId != null || testingId === profile.id || modelsLoadingId === profile.id}
              testing={testingId === profile.id}
              loadingModels={modelsLoadingId === profile.id}
              modelOptions={modelOptions[profile.id] ?? []}
              testResult={testResults[profile.id]}
              healthStale={staleHealthIds.has(profile.id)}
              oauthStatus={oauthStatus}
              oauthLoading={oauthLoading}
              index={index}
              total={profiles.length}
              dragging={draggingId === profile.id}
              dropTarget={dropTargetId === profile.id && draggingId !== profile.id}
              onToggleExpanded={() => setExpandedId((current) => current === profile.id ? null : profile.id)}
              onChange={(patch) => updateProfile(profile.id, patch)}
              onToggleEnabled={(enabled) => toggleProfile(profile.id, enabled)}
              onTest={() => testProfile(profile)}
              onFetchModels={() => fetchModels(profile)}
              onDuplicate={() => duplicateProfile(profile)}
              onDelete={() => deleteProfile(profile.id)}
              onMove={(direction) => moveProfile(profile.id, direction)}
              onDragStart={(event: DragEvent<HTMLElement>) => {
                event.dataTransfer.effectAllowed = "move";
                event.dataTransfer.setData("text/plain", profile.id);
                setDraggingId(profile.id);
              }}
              onDragOver={(event: DragEvent<HTMLElement>) => {
                if (!draggingId || draggingId === profile.id) return;
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
                setDropTargetId(profile.id);
              }}
              onDrop={(event: DragEvent<HTMLElement>) => {
                event.preventDefault();
                void dropProfile(profile.id);
              }}
              onDragEnd={() => {
                setDraggingId(null);
                setDropTargetId(null);
              }}
              onAddCredential={(label, value) => addCredential(profile.id, label, value)}
              onReplaceCredential={(id, value) => replaceCredential(profile.id, id, value)}
              onToggleCredential={(id, enabled) => toggleCredential(profile.id, id, enabled)}
              onDeleteCredential={(id) => deleteCredential(profile.id, id)}
              onReorderCredentials={(ids) => reorderCredentials(profile.id, ids)}
              onOAuthLogin={() => loginWithOpenAi(profile)}
              onOAuthLogout={logoutFromOpenAi}
            />
          ))}
        </div>
      )}
    </div>
  );
}
