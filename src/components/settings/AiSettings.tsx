import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, KeyRound, Loader2, Plus, RotateCw, Shield, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "../ui/Button";
import Select from "../ui/Select";
import Input from "../ui/Input";
import Slider from "../ui/Slider";
import Toggle from "../ui/Toggle";
import type { SettingsProps } from "./types";

interface AiSettingsProps extends SettingsProps {
  onSaveRef?: (save: (() => void) | null) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

interface AiProfile {
  id: string;
  label: string;
  provider: string;
  auth_mode: "api_key" | "oauth";
  base_url: string | null;
  model: string;
  temperature: number;
  keep_alive: string | null;
}

interface AiCredential {
  id: string;
  profile_id: string;
  label: string;
  masked_suffix: string;
  enabled: boolean;
  priority: number;
  state: string;
  cooldown_until: number | null;
  last_error_kind: string | null;
}

export default function AiSettings({ showSavedToast, onSaveRef, onDirtyChange }: AiSettingsProps) {
  const { t } = useTranslation();
  const [profile, setProfile] = useState<AiProfile | null>(null);
  const [credentials, setCredentials] = useState<AiCredential[]>([]);
  const [dirty, setDirty] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [newLabel, setNewLabel] = useState("");
  const [replaceId, setReplaceId] = useState<string | null>(null);
  const [replaceValue, setReplaceValue] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [oauthStatus, setOauthStatus] = useState<{ connected: boolean; account_id: string | null }>({ connected: false, account_id: null });
  const [oauthLoading, setOauthLoading] = useState(false);

  const load = useCallback(async () => {
    try {
      const nextProfile = await invoke<AiProfile>("ai_active_profile");
      const nextCredentials = await invoke<AiCredential[]>("ai_list_credentials", { profileId: nextProfile.id });
      setProfile(nextProfile);
      setCredentials(nextCredentials);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => { load(); }, [load]);
  useEffect(() => { onDirtyChange?.(dirty); }, [dirty, onDirtyChange]);

  const refreshOAuthStatus = useCallback(async () => {
    try {
      setOauthStatus(await invoke<{ connected: boolean; account_id: string | null }>("openai_oauth_status"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    if (profile?.provider === "openai" && profile.auth_mode === "oauth") {
      refreshOAuthStatus();
    }
  }, [profile?.auth_mode, profile?.provider, refreshOAuthStatus]);

  const updateProfile = (patch: Partial<AiProfile>) => {
    setProfile((current) => current ? { ...current, ...patch } : current);
    setDirty(true);
  };

  const saveProfile = useCallback(async () => {
    if (!profile) return;
    try {
      const saved = await invoke<AiProfile>("ai_save_profile", {
        id: profile.id,
        label: profile.label,
        provider: profile.provider,
        authMode: profile.auth_mode,
        baseUrl: profile.base_url || null,
        model: profile.model,
        temperature: profile.temperature,
        keepAlive: profile.keep_alive || null,
      });
      setProfile(saved);
      setDirty(false);
      showSavedToast(t("settings.ai.savedToast"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [profile, showSavedToast, t]);

  useEffect(() => {
    onSaveRef?.(saveProfile);
    return () => onSaveRef?.(null);
  }, [onSaveRef, saveProfile]);

  const addCredential = async () => {
    if (!profile || !newKey.trim()) return;
    setBusyId("new");
    setError(null);
    try {
      await invoke("ai_add_credential", {
        profileId: profile.id,
        label: newLabel.trim() || t("settings.ai.defaultKeyLabel"),
        value: newKey.trim(),
      });
      setNewKey("");
      setNewLabel("");
      await load();
      showSavedToast(t("settings.ai.keyAdded"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const replaceCredential = async () => {
    if (!replaceId || !replaceValue.trim()) return;
    setBusyId(replaceId);
    try {
      await invoke("ai_replace_credential", { id: replaceId, value: replaceValue.trim() });
      setReplaceId(null);
      setReplaceValue("");
      await load();
      showSavedToast(t("settings.ai.keyReplaced"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const mutateCredential = async (id: string, action: "toggle" | "delete" | "test", enabled?: boolean) => {
    setBusyId(id);
    setError(null);
    try {
      if (action === "toggle") await invoke("ai_set_credential_enabled", { id, enabled });
      if (action === "delete") await invoke("ai_delete_credential", { id });
      if (action === "test") await invoke("ai_test_credential", { id });
      await load();
      if (action === "test") showSavedToast(t("settings.ai.keyTested"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const reorder = async (index: number, direction: -1 | 1) => {
    const next = [...credentials];
    const target = index + direction;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    setCredentials(next);
    try {
      await invoke("ai_reorder_credentials", { ids: next.map((credential) => credential.id) });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      load();
    }
  };

  const loginWithOpenAi = async () => {
    if (!profile) return;
    setOauthLoading(true);
    setError(null);
    try {
      // OAuth must use the persisted profile. Saving here prevents the
      // browser flow from authenticating an API-key profile by accident.
      const saved = await invoke<AiProfile>("ai_save_profile", {
        id: profile.id,
        label: profile.label,
        provider: profile.provider,
        authMode: "oauth",
        baseUrl: null,
        model: profile.model,
        temperature: profile.temperature,
        keepAlive: profile.keep_alive || null,
      });
      setProfile(saved);
      setDirty(false);
      const status = await invoke<{ connected: boolean; account_id: string | null }>("openai_oauth_login");
      setOauthStatus(status);
      showSavedToast(t("settings.ai.oauthSuccess"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
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
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setOauthLoading(false);
    }
  };

  if (!profile) {
    return <div className="flex items-center gap-2 py-6 text-[13px] text-text-muted"><Loader2 size={16} className="animate-spin" />{t("settings.librarySync.working")}</div>;
  }

  const usesApiKeys = profile.auth_mode === "api_key" && profile.provider !== "ollama";
  return (
    <div className="space-y-0">
      <div className="py-3 border-b border-border flex items-center justify-between gap-4">
        <div><p className="text-[14px] font-medium text-text-primary">{t("settings.ai.provider")}</p><p className="text-[12px] text-text-muted mt-0.5">{t("settings.ai.providerHint")}</p></div>
        <Select className="w-[160px] shrink-0" value={profile.provider} onChange={(provider) => {
          const defaults: Record<string, Partial<AiProfile>> = {
            ollama: { base_url: "http://localhost:11434", model: "qwen3.5", auth_mode: "api_key" },
            openai: { base_url: "https://api.openai.com", model: "gpt-4o-mini", auth_mode: "api_key" },
            anthropic: { base_url: "https://api.anthropic.com", model: "claude-sonnet-4-20250514", auth_mode: "api_key" },
            custom: { base_url: "", model: "", auth_mode: "api_key" },
          };
          updateProfile({ provider, ...defaults[provider] });
        }} options={[
          { value: "openai", label: "OpenAI" }, { value: "custom", label: t("settings.ai.customCompatible") },
          { value: "anthropic", label: "Anthropic" }, { value: "ollama", label: "Ollama (Local)" },
        ]} />
      </div>

      {profile.provider === "openai" && <div className="py-3 border-b border-border"><p className="text-[14px] font-medium text-text-primary mb-1.5">{t("settings.ai.authMethod")}</p><div className="flex border border-border rounded-lg overflow-hidden"><button type="button" onClick={() => updateProfile({ auth_mode: "api_key" })} className={`flex-1 h-9 text-[13px] ${profile.auth_mode === "api_key" ? "bg-accent text-white" : "bg-bg-page text-text-secondary"}`}><KeyRound size={14} className="inline mr-1.5" />{t("settings.ai.apiKey")}</button><button type="button" onClick={() => updateProfile({ auth_mode: "oauth", model: "gpt-5.3-codex" })} className={`flex-1 h-9 text-[13px] ${profile.auth_mode === "oauth" ? "bg-accent text-white" : "bg-bg-page text-text-secondary"}`}><Shield size={14} className="inline mr-1.5" />{t("settings.ai.oauthLogin")}</button></div></div>}

      {profile.provider !== "openai" || profile.auth_mode === "api_key" ? <div className="py-3 border-b border-border"><p className="text-[14px] font-medium text-text-primary mb-1.5">{t("settings.ai.baseUrl")}</p><Input value={profile.base_url ?? ""} onChange={(event) => updateProfile({ base_url: event.target.value })} placeholder="https://api.example.com" /></div> : <div className="py-3 border-b border-border space-y-3"><p className="text-[12px] text-text-muted">{t("settings.ai.oauthUsesAccount")}</p><p className="text-[12px] text-text-muted">{t("settings.ai.oauthHint")}</p>{oauthStatus.connected ? <div className="flex items-center justify-between gap-3"><span className="min-w-0 truncate text-[13px] text-text-primary">{t("settings.ai.connected", { account: oauthStatus.account_id || "OpenAI" })}</span><Button variant="ghost" size="sm" onClick={logoutFromOpenAi} disabled={oauthLoading}>{oauthLoading ? <Loader2 size={14} className="animate-spin" /> : null}{t("settings.ai.logout")}</Button></div> : <Button variant="primary" size="sm" onClick={loginWithOpenAi} disabled={oauthLoading}>{oauthLoading ? <Loader2 size={14} className="animate-spin" /> : <Shield size={14} />}{oauthLoading ? t("settings.ai.waitingAuth") : t("settings.ai.loginWithOpenAI")}</Button>}</div>}
      <div className="py-3 border-b border-border"><p className="text-[14px] font-medium text-text-primary mb-1.5">{t("settings.ai.model")}</p><Input value={profile.model} onChange={(event) => updateProfile({ model: event.target.value })} /></div>
      <div className="py-3 border-b border-border"><Slider label={t("settings.ai.temperature")} min={0} max={100} value={Math.round(profile.temperature * 100)} onChange={(value) => updateProfile({ temperature: value / 100 })} displayValue={profile.temperature.toFixed(1)} hint={t("settings.ai.temperatureHint")} /></div>

      {usesApiKeys && <div className="py-4 border-b border-border">
        <div className="flex items-center justify-between mb-3"><div><p className="text-[14px] font-medium text-text-primary">{t("settings.ai.apiKeys")}</p><p className="text-[12px] text-text-muted mt-0.5">{t("settings.ai.apiKeysHint")}</p></div><span className="text-[12px] text-text-muted">{credentials.filter((credential) => credential.enabled).length}</span></div>
        <div className="space-y-2">
          {credentials.map((credential, index) => <div key={credential.id} className="border border-border rounded-lg px-3 py-2.5">
            <div className="flex items-center gap-2"><Toggle checked={credential.enabled} onChange={(enabled) => mutateCredential(credential.id, "toggle", enabled)} /><div className="flex-1 min-w-0"><p className="text-[13px] font-medium text-text-primary truncate">{credential.label} <span className="font-mono text-text-muted">••••{credential.masked_suffix}</span></p><p className="text-[11px] text-text-muted">{credential.state === "active" ? t("settings.ai.keyActive") : t("settings.ai.keyState", { state: credential.state })}</p></div><button type="button" title={t("settings.ai.moveUp")} disabled={index === 0} onClick={() => reorder(index, -1)} className="p-1 text-text-muted disabled:opacity-30"><ArrowUp size={14} /></button><button type="button" title={t("settings.ai.moveDown")} disabled={index === credentials.length - 1} onClick={() => reorder(index, 1)} className="p-1 text-text-muted disabled:opacity-30"><ArrowDown size={14} /></button><button type="button" title={t("settings.ai.testKey")} onClick={() => mutateCredential(credential.id, "test")} className="p-1 text-text-muted hover:text-accent-text"><RotateCw size={14} /></button><button type="button" title={t("settings.ai.deleteKey")} onClick={() => mutateCredential(credential.id, "delete")} className="p-1 text-text-muted hover:text-danger-text"><Trash2 size={14} /></button></div>
            {replaceId === credential.id ? <div className="flex gap-2 mt-2"><Input type="password" value={replaceValue} onChange={(event) => setReplaceValue(event.target.value)} placeholder="sk-..." /><Button size="sm" variant="primary" onClick={replaceCredential} disabled={busyId === credential.id}>{t("settings.ai.replaceKey")}</Button></div> : <button type="button" onClick={() => setReplaceId(credential.id)} className="text-[12px] text-accent-text mt-2">{t("settings.ai.replaceKey")}</button>}
          </div>)}
        </div>
        <div className="mt-3 grid grid-cols-[1fr_1.4fr_auto] gap-2"><Input value={newLabel} onChange={(event) => setNewLabel(event.target.value)} placeholder={t("settings.ai.keyLabel")} /><Input type="password" value={newKey} onChange={(event) => setNewKey(event.target.value)} placeholder="sk-..." /><Button variant="primary" size="sm" onClick={addCredential} disabled={!newKey.trim() || busyId === "new"}><Plus size={14} />{t("settings.ai.addKey")}</Button></div>
      </div>}

      {error && <p className="mt-3 text-[12px] text-danger-text">{error}</p>}
    </div>
  );
}
