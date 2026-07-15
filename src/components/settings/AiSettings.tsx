import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Activity, AlertCircle, Loader2, Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "../ui/Button";
import Input from "../ui/Input";
import Select from "../ui/Select";
import Toggle from "../ui/Toggle";
import AiServiceCard, {
  type AiConnectionTestResult,
  type AiCredential,
  type AiProfile,
} from "./AiServiceCard";
import type { SettingsProps } from "./types";
import { invokeWithCredentialMigration } from "../../utils/vaultAccess";
import { useSettings } from "../../hooks/useSettings";

interface AiSettingsProps extends SettingsProps {
  onSaveRef?: (save: (() => void) | null) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

interface OAuthStatus {
  connected: boolean;
  account_id: string | null;
}

interface VaultStatus {
  encryptedSecretCount: number;
  legacyKeychainCandidateCount: number;
  pendingMigrationCount: number;
}

interface VectorAvailability {
  available: boolean;
  reason: string | null;
  dimensions?: number | null;
  model?: string | null;
}

interface EmbeddingProbeResult {
  ok: boolean;
  dimensions: number;
  latencyMs: number;
  error?: string | null;
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

function isProfileConfigValid(profile: AiProfile): boolean {
  const label = profile.label.trim();
  const model = profile.model.trim();
  if (!label || Array.from(label).length > 100) return false;
  if (!model || Array.from(model).length > 200) return false;
  if (!Number.isFinite(profile.temperature) || profile.temperature < 0 || profile.temperature > 2) return false;
  if (!(["openai", "anthropic", "ollama", "custom"] as string[]).includes(profile.provider)) return false;
  if (profile.auth_mode === "oauth" && profile.provider !== "openai") return false;
  const baseUrl = profile.base_url?.trim();
  if (profile.provider === "custom" && !baseUrl) return false;
  if (baseUrl) {
    try {
      const parsed = new URL(baseUrl);
      if (!(["http:", "https:"] as string[]).includes(parsed.protocol) || !parsed.hostname) return false;
    } catch {
      return false;
    }
  }
  return true;
}

function updateOne<T extends { id: string }>(items: T[], id: string, patch: Partial<T>): T[] {
  return items.map((item) => item.id === id ? { ...item, ...patch } : item);
}

export default function AiSettings({ showSavedToast, onSaveRef, onDirtyChange }: AiSettingsProps) {
  const { t } = useTranslation();
  const { settings, save: saveSetting } = useSettings();
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
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [migratingCredentials, setMigratingCredentials] = useState(false);
  const [vectorAvailability, setVectorAvailability] = useState<VectorAvailability>({
    available: false,
    reason: "requires_compatible_provider",
  });
  const [embeddingEndpoint, setEmbeddingEndpoint] = useState("");
  const [embeddingModel, setEmbeddingModel] = useState("");
  const [embeddingKey, setEmbeddingKey] = useState("");
  const [embeddingTesting, setEmbeddingTesting] = useState(false);
  const [embeddingProbe, setEmbeddingProbe] = useState<EmbeddingProbeResult | null>(null);
  const autoSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const profilesRef = useRef<AiProfile[]>([]);
  const savedProfilesRef = useRef<AiProfile[]>([]);
  const saveInFlightRef = useRef<Promise<void> | null>(null);
  const saveRequestedRef = useRef(false);
  const saveNotificationRequestedRef = useRef(false);
  const flushOnUnmountRef = useRef<() => void>(() => {});
  const mountedRef = useRef(false);

  const replaceProfiles = useCallback((next: AiProfile[]) => {
    profilesRef.current = next;
    setProfiles(next);
  }, []);

  const replaceSavedProfiles = useCallback((next: AiProfile[]) => {
    savedProfilesRef.current = next;
    setSavedProfiles(next);
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      flushOnUnmountRef.current();
    };
  }, []);

  const dirtyIds = useMemo(() => {
    const saved = new Map(savedProfiles.map((profile) => [profile.id, profile]));
    return new Set(profiles.filter((profile) => !sameProfileConfig(profile, saved.get(profile.id))).map((profile) => profile.id));
  }, [profiles, savedProfiles]);

  const validDirtyIds = useMemo(() => new Set(
    profiles
      .filter((profile) => dirtyIds.has(profile.id) && isProfileConfigValid(profile))
      .map((profile) => profile.id),
  ), [dirtyIds, profiles]);

  const refreshCredentials = useCallback(async (profileId: string) => {
    const next = await invoke<AiCredential[]>("ai_list_credentials", { profileId });
    setCredentials((current) => ({ ...current, [profileId]: next }));
    return next;
  }, []);

  const refreshOAuthStatus = useCallback(async () => {
    const next = await invoke<OAuthStatus>("openai_oauth_status");
    setOauthStatus(next);
  }, []);

  const refreshVaultStatus = useCallback(async () => {
    const next = await invoke<VaultStatus>("vault_status");
    setVaultStatus(next);
  }, []);

  const refreshVectorAvailability = useCallback(async () => {
    const next = await invoke<VectorAvailability>("ai_vector_retrieval_status");
    setVectorAvailability(next);
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
      replaceProfiles(nextProfiles);
      replaceSavedProfiles(nextProfiles);
      setEmbeddingEndpoint(settings.ai_embedding_endpoint || "http://localhost:11434/v1/embeddings");
      setEmbeddingModel(settings.ai_embedding_model || "text-embedding-3-small");
      setCredentials(Object.fromEntries(credentialEntries));
      setExpandedId((current) => current && nextProfiles.some((profile) => profile.id === current) ? current : null);
      try {
        await refreshOAuthStatus();
      } catch {
        // OAuth is optional; profile and API-key configuration remain usable.
      }
      try {
        await refreshVaultStatus();
      } catch {
        // The migration reminder is informational and must not block AI setup.
      }
      try {
        await refreshVectorAvailability();
      } catch {
        setVectorAvailability({ available: false, reason: "requires_compatible_provider" });
      }
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setLoading(false);
    }
  }, [refreshOAuthStatus, refreshVaultStatus, refreshVectorAvailability, replaceProfiles, replaceSavedProfiles, settings.ai_embedding_endpoint, settings.ai_embedding_model]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    onDirtyChange?.(dirtyIds.size > 0);
  }, [dirtyIds, onDirtyChange]);

  const updateProfile = useCallback((id: string, patch: Partial<AiProfile>) => {
    const nextProfiles = updateOne(profilesRef.current, id, patch);
    replaceProfiles(nextProfiles);
    if (["provider", "auth_mode", "base_url", "model", "temperature", "keep_alive"].some((key) => key in patch)) {
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
  }, [replaceProfiles]);

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
    const nextSavedProfiles = updateOne(savedProfilesRef.current, saved.id, saved);
    savedProfilesRef.current = nextSavedProfiles;
    // A debounced save may resolve after the user has resumed typing. Only
    // replace the draft when it is still the exact revision we persisted.
    if (mountedRef.current) {
      const nextProfiles = profilesRef.current.map((item) => (
        item.id === saved.id && sameProfileConfig(item, profile) ? saved : item
      ));
      replaceProfiles(nextProfiles);
      replaceSavedProfiles(nextSavedProfiles);
    }
    return saved;
  }, [replaceProfiles, replaceSavedProfiles]);

  const saveProfiles = useCallback((notify = true): Promise<void> => {
    saveRequestedRef.current = true;
    saveNotificationRequestedRef.current ||= notify;
    if (saveInFlightRef.current) return saveInFlightRef.current;

    const worker = (async () => {
      let savedAny = false;
      if (mountedRef.current) {
        setSaving(true);
        setError(null);
      }
      try {
        while (saveRequestedRef.current) {
          saveRequestedRef.current = false;
          const savedById = new Map(savedProfilesRef.current.map((profile) => [profile.id, profile]));
          const pending = profilesRef.current.filter((profile) => (
            !sameProfileConfig(profile, savedById.get(profile.id)) && isProfileConfigValid(profile)
          ));
          for (const profile of pending) {
            await persistProfile(profile);
            savedAny = true;
          }
        }
        if (savedAny && saveNotificationRequestedRef.current && mountedRef.current) {
          showSavedToast(t("settings.ai.savedToast"));
        }
      } catch (nextError) {
        saveRequestedRef.current = false;
        if (mountedRef.current) setError(errorText(nextError));
      } finally {
        saveNotificationRequestedRef.current = false;
        if (mountedRef.current) setSaving(false);
      }
    })();

    saveInFlightRef.current = worker;
    void worker.finally(() => {
      saveInFlightRef.current = null;
    });
    return worker;
  }, [persistProfile, showSavedToast, t]);

  useEffect(() => {
    flushOnUnmountRef.current = () => {
      void saveProfiles(false);
    };
  }, [saveProfiles]);

  const requestSave = useCallback(() => {
    void saveProfiles(true);
  }, [saveProfiles]);

  useEffect(() => {
    onSaveRef?.(requestSave);
    return () => onSaveRef?.(null);
  }, [onSaveRef, requestSave]);

  useEffect(() => {
    if (autoSaveTimerRef.current) clearTimeout(autoSaveTimerRef.current);
    if (loading || saving || validDirtyIds.size === 0) return;
    autoSaveTimerRef.current = setTimeout(() => {
      autoSaveTimerRef.current = null;
      void saveProfiles(false);
    }, 600);
    return () => {
      if (autoSaveTimerRef.current) {
        clearTimeout(autoSaveTimerRef.current);
        autoSaveTimerRef.current = null;
      }
    };
  }, [loading, saveProfiles, saving, validDirtyIds]);

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
      replaceProfiles([...profilesRef.current, created]);
      replaceSavedProfiles([...savedProfilesRef.current, created]);
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
      replaceProfiles([...profilesRef.current, configured]);
      replaceSavedProfiles([...savedProfilesRef.current, configured]);
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
      replaceProfiles(profilesRef.current.filter((profile) => profile.id !== id));
      replaceSavedProfiles(savedProfilesRef.current.filter((profile) => profile.id !== id));
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
    const previous = profilesRef.current.find((profile) => profile.id === id)?.enabled ?? !enabled;
    setBusyId(id);
    replaceProfiles(updateOne(profilesRef.current, id, { enabled }));
    replaceSavedProfiles(updateOne(savedProfilesRef.current, id, { enabled }));
    setError(null);
    try {
      await invoke("ai_set_profile_enabled", { id, enabled });
    } catch (nextError) {
      replaceProfiles(updateOne(profilesRef.current, id, { enabled: previous }));
      replaceSavedProfiles(updateOne(savedProfilesRef.current, id, { enabled: previous }));
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  };

  const applyProfileOrder = useCallback(async (next: AiProfile[]) => {
    const previousProfiles = profilesRef.current;
    const previousSaved = savedProfilesRef.current;
    const withPriority = next.map((profile, priority) => ({ ...profile, priority }));
    const nextSaved = withPriority.map((profile) => {
      const saved = previousSaved.find((item) => item.id === profile.id);
      return saved ? { ...saved, priority: profile.priority } : profile;
    });
    replaceProfiles(withPriority);
    replaceSavedProfiles(nextSaved);
    setBusyId("order");
    setError(null);
    try {
      await invoke("ai_reorder_profiles", { ids: withPriority.map((profile) => profile.id) });
    } catch (nextError) {
      replaceProfiles(previousProfiles);
      replaceSavedProfiles(previousSaved);
      setError(errorText(nextError));
    } finally {
      setBusyId(null);
    }
  }, [replaceProfiles, replaceSavedProfiles]);

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
      const latestProfile = profilesRef.current.find((item) => item.id === profile.id) ?? profile;
      const savedProfile = savedProfilesRef.current.find((item) => item.id === profile.id);
      if (!sameProfileConfig(latestProfile, savedProfile) && isProfileConfigValid(latestProfile)) {
        await saveProfiles(false);
      }
      const testedProfile = profilesRef.current.find((item) => item.id === profile.id) ?? latestProfile;
      const result = await invoke<AiConnectionTestResult>("ai_test_profile", {
        id: testedProfile.id,
        provider: testedProfile.provider,
        authMode: testedProfile.auth_mode,
        baseUrl: testedProfile.base_url?.trim() || null,
        model: testedProfile.model,
        temperature: testedProfile.temperature,
        keepAlive: testedProfile.keep_alive?.trim() || null,
      });
      setTestResults((current) => ({ ...current, [profile.id]: result }));
      setStaleHealthIds((current) => {
        const next = new Set(current);
        next.delete(profile.id);
        return next;
      });
      try {
        const [nextProfiles] = await Promise.all([
          invoke<AiProfile[]>("ai_list_profiles"),
          refreshCredentials(testedProfile.id),
        ]);
        const persisted = nextProfiles.find((item) => item.id === testedProfile.id);
        if (persisted) {
          const health = {
            state: persisted.state,
            cooldown_until: persisted.cooldown_until,
            last_error_kind: persisted.last_error_kind,
            last_used_at: persisted.last_used_at,
            last_latency_ms: persisted.last_latency_ms,
          };
          // Preserve unsaved form fields while refreshing only authoritative
          // health metadata written by a test of the saved configuration.
          replaceProfiles(updateOne(profilesRef.current, testedProfile.id, health));
          replaceSavedProfiles(updateOne(savedProfilesRef.current, testedProfile.id, health));
        }
      } catch (refreshError) {
        setError(errorText(refreshError));
      }
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
      const status = await invoke<OAuthStatus>("openai_oauth_login");
      setOauthStatus(status);
      replaceProfiles(updateOne(profilesRef.current, oauthProfile.id, oauthProfile));
      await saveProfiles(false);
      markHealthStale(profile.id);
      showSavedToast(t("settings.ai.oauthSuccess"));
    } catch (nextError) {
      setError(errorText(nextError));
    } finally {
      setOauthLoading(false);
    }
  };

  const migrateCredentials = async () => {
    setMigratingCredentials(true);
    setError(null);
    try {
      await invokeWithCredentialMigration<number>("vault_migrate_to_local");
      await refreshVaultStatus();
      showSavedToast(t("settings.ai.pendingCredentialsSecured"));
    } catch (nextError) {
      const code = errorText(nextError);
      if (code === "VAULT_USER_CANCELLED") return;
      const partial = code.match(/VAULT_PARTIAL_MIGRATION:imported=(\d+):pending=(\d+):/);
      if (partial) {
        await refreshVaultStatus().catch(() => {});
        setError(t("settings.ai.credentialMigrationPartial", {
          imported: Number(partial[1]),
          pending: Number(partial[2]),
        }));
      } else if (code.includes("VAULT_MASTER_KEY_MISSING")) {
        setError(t("settings.ai.credentialMigrationMasterMissing"));
      } else if (code.includes("VAULT_ACCESS_DENIED")) {
        setError(t("settings.ai.credentialMigrationDenied"));
      } else if (code.includes("VAULT_DATA_CORRUPT") || code.includes("VAULT_MASTER_KEY_INVALID")) {
        setError(t("settings.ai.credentialMigrationCorrupt"));
      } else if (code.includes("VAULT_ACCESS_UNAVAILABLE")) {
        setError(t("settings.ai.credentialMigrationUnavailable"));
      } else {
        setError(code);
      }
    } finally {
      setMigratingCredentials(false);
    }
  };

  const logoutFromOpenAi = async () => {
    setOauthLoading(true);
    setError(null);
    try {
      await invoke("openai_oauth_logout");
      setOauthStatus({ connected: false, account_id: null });
      const affectedIds = profilesRef.current
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

  const toggleVectorRetrieval = async (enabled: boolean) => {
    setError(null);
    try {
      await invoke("set_ai_vector_retrieval", { enabled });
      await saveSetting("ai_vector_retrieval", enabled ? "true" : "false");
      await refreshVectorAvailability();
    } catch (nextError) {
      setError(errorText(nextError));
      await refreshVectorAvailability().catch(() => {});
    }
  };

  const testEmbedding = async () => {
    setEmbeddingTesting(true);
    setError(null);
    try {
      const result = await invoke<EmbeddingProbeResult>("ai_embedding_probe", {
        endpoint: embeddingEndpoint,
        model: embeddingModel,
        apiKey: embeddingKey || null,
      });
      setEmbeddingProbe(result);
      if (result.ok) {
        setEmbeddingKey("");
        await refreshVectorAvailability();
      }
    } catch (nextError) {
      setEmbeddingProbe(null);
      setError(errorText(nextError));
    } finally {
      setEmbeddingTesting(false);
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
      <div className="mb-3">
        <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.chatModels")}</h4>
        <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.chatModelsHint")}</p>
      </div>
      <div className="mb-4 border-y border-border py-4">
        <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.embeddingTitle")}</h4>
        <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.embeddingHint")}</p>
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          <Input value={embeddingEndpoint} onChange={(event) => setEmbeddingEndpoint(event.target.value)} placeholder="http://localhost:11434/v1/embeddings" />
          <Input value={embeddingModel} onChange={(event) => setEmbeddingModel(event.target.value)} placeholder="text-embedding-3-small" />
        </div>
        <div className="mt-2 flex items-center gap-2">
          <Input className="min-w-0 flex-1" type="password" value={embeddingKey} onChange={(event) => setEmbeddingKey(event.target.value)} placeholder={t("settings.ai.embeddingKeyPlaceholder")} />
          <Button variant="secondary" size="sm" onClick={() => void testEmbedding()} disabled={embeddingTesting || !embeddingEndpoint.trim() || !embeddingModel.trim()}>
            {embeddingTesting ? <Loader2 size={14} className="animate-spin" /> : <Activity size={14} />}
            {t("settings.ai.embeddingTest")}
          </Button>
        </div>
        {(embeddingProbe || vectorAvailability.available) && (
          <p className={`mt-2 text-[11px] ${embeddingProbe?.ok === false ? "text-danger-text" : "text-success-text"}`}>
            {embeddingProbe?.ok === false
              ? t("settings.ai.embeddingFailed")
              : t("settings.ai.embeddingAvailable", {
                  dimensions: embeddingProbe?.dimensions ?? vectorAvailability.dimensions,
                  latency: embeddingProbe?.latencyMs ?? "-",
                })}
          </p>
        )}
      </div>
      <div className="mb-4 flex min-h-[73px] items-center justify-between gap-4 border-b border-border py-3">
        <div className="min-w-0">
          <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.grounding")}</h4>
          <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.groundingHint")}</p>
        </div>
        <Toggle
          checked={settings.ai_grounding_enabled !== "false"}
          onChange={(enabled) => void saveSetting("ai_grounding_enabled", enabled ? "true" : "false")}
          label={t("settings.ai.grounding")}
        />
      </div>
      <div className="mb-4 flex min-h-[73px] items-center justify-between gap-4 border-b border-border py-3">
        <div className="min-w-0">
          <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.spoilerGuard")}</h4>
          <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.spoilerGuardHint")}</p>
        </div>
        <Toggle
          checked={settings.ai_spoiler_guard !== "false"}
          onChange={(enabled) => void saveSetting("ai_spoiler_guard", enabled ? "true" : "false")}
          label={t("settings.ai.spoilerGuard")}
        />
      </div>
      <div className="mb-4 border-b border-border py-3">
        <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.summaryProfile")}</h4>
        <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.summaryProfileHint")}</p>
        <Select
          className="mt-2"
          value={settings.ai_summary_profile_id || ""}
          onChange={(value) => void saveSetting("ai_summary_profile_id", value)}
          options={[
            { value: "", label: t("settings.ai.summaryProfileFollow") },
            ...profiles.filter((profile) => profile.enabled).map((profile) => ({ value: profile.id, label: profile.label })),
          ]}
        />
      </div>
      <div className="mb-4 flex min-h-[73px] items-center justify-between gap-4 border-b border-border py-3">
        <div className="min-w-0">
          <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.vectorRetrieval")}</h4>
          <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">
            {vectorAvailability.available
              ? t("settings.ai.vectorRetrievalHint")
              : t("settings.ai.vectorRetrievalUnavailable")}
          </p>
        </div>
        <Toggle
          checked={settings.ai_vector_retrieval === "true"}
          onChange={(enabled) => void toggleVectorRetrieval(enabled)}
          disabled={!vectorAvailability.available}
          label={t("settings.ai.vectorRetrieval")}
        />
      </div>
      <div className="mb-4 flex min-h-[73px] items-center justify-between gap-4 border-b border-border py-3">
        <div className="min-w-0">
          <h4 className="text-[13px] font-medium text-text-primary">{t("settings.ai.summariesAuto")}</h4>
          <p className="mt-0.5 text-[11px] leading-[1.55] text-text-muted">{t("settings.ai.summariesAutoHint")}</p>
        </div>
        <Toggle
          checked={settings.ai_summaries_auto !== "false"}
          onChange={(enabled) => void saveSetting("ai_summaries_auto", enabled ? "true" : "false")}
          label={t("settings.ai.summariesAuto")}
        />
      </div>
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

      {(vaultStatus?.pendingMigrationCount ?? 0) > 0 && (
        <div role="status" className="mb-3 flex items-center justify-between gap-3 rounded-md border border-amber-300/70 bg-amber-50 px-3 py-2 text-[11px] leading-5 text-amber-950 dark:border-amber-500/35 dark:bg-amber-950/25 dark:text-amber-100">
          <div className="min-w-0">
            <p className="font-medium">{t("settings.ai.pendingCredentialsTitle", { count: vaultStatus?.pendingMigrationCount ?? 0 })}</p>
            <p className="text-amber-900/80 dark:text-amber-100/75">{t("settings.ai.pendingCredentialsHint")}</p>
          </div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void migrateCredentials()}
            disabled={migratingCredentials || busyId != null || saving}
            className="shrink-0"
          >
            {migratingCredentials ? <Loader2 size={14} className="animate-spin" /> : null}
            {t("settings.ai.pendingCredentialsAction")}
          </Button>
        </div>
      )}

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
          {profiles.map((profile) => (
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
