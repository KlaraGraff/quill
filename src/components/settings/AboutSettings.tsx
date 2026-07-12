import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Github, BookText, Scale, ExternalLink, GitFork, Bug, Check, Copy } from "lucide-react";
import QuillLogo from "../QuillLogo";

const CURRENT_REPOSITORY_URL = "https://github.com/KlaraGraff/quill";
const CURRENT_RELEASES_URL = `${CURRENT_REPOSITORY_URL}/releases`;
const CURRENT_ISSUES_URL = `${CURRENT_REPOSITORY_URL}/issues`;
const CURRENT_DOCS_URL = `${CURRENT_REPOSITORY_URL}#readme`;
const UPSTREAM_REPOSITORY_URL = "https://github.com/yicheng47/quill";

interface BuildInfo {
  version: string;
  upstream_baseline: string;
  commit: string;
  built_at: string;
  channel: string;
  bundle_identifier: string;
  repository: string;
  upstream_repository: string;
}

// Informational platform label derived from the UA string (no os plugin).
function platformLabel(): string {
  if (typeof navigator === "undefined") return "";
  const ua = navigator.userAgent.toLowerCase();
  const arch = ua.includes("arm64") || ua.includes("aarch64")
    ? "arm64"
    : ua.includes("x86_64") || ua.includes("x64")
      ? "x86_64"
      : "";
  const os = ua.includes("mac")
    ? "macOS"
    : ua.includes("win")
      ? "Windows"
      : ua.includes("linux")
        ? "Linux"
        : "";
  if (!os) return "";
  return arch ? `${os} · ${arch}` : os;
}

export default function AboutSettings() {
  const { t } = useTranslation();
  const [buildInfo, setBuildInfo] = useState<BuildInfo | null>(null);
  const [copied, setCopied] = useState(false);
  const platform = platformLabel();

  useEffect(() => {
    invoke<BuildInfo>("app_build_info").then(setBuildInfo).catch(() => setBuildInfo(null));
  }, []);

  const open = (url: string) => {
    openUrl(url).catch(() => {});
  };

  const copyDiagnostics = async () => {
    if (!buildInfo) return;
    const details = [
      `Quill Personal ${buildInfo.version}`,
      `Upstream baseline: ${buildInfo.upstream_baseline}`,
      `Commit: ${buildInfo.commit}`,
      `Built: ${buildInfo.built_at}`,
      `Channel: ${buildInfo.channel}`,
      `Platform: ${platform || "unknown"}`,
      `Bundle ID: ${buildInfo.bundle_identifier}`,
    ].join("\n");
    try {
      await navigator.clipboard.writeText(details);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="flex flex-col min-h-full pb-2">
      {/* Identity */}
      <div className="flex flex-col items-center gap-3.5 pt-4 pb-6">
        <QuillLogo size={56} className="rounded-2xl" />
        <div className="flex flex-col items-center gap-1.5">
          <span className="text-[20px] font-semibold text-text-primary tracking-[0.5px]">
            Quill Personal
          </span>
          <span className="text-[12px] text-text-muted">{t("settings.about.description")}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="bg-bg-page dark:bg-bg-input text-text-secondary text-[12px] font-mono px-2 py-0.5 rounded-lg">
            v{buildInfo?.version ?? "..."}
          </span>
          {platform && (
            <span className="bg-bg-page dark:bg-bg-input text-text-secondary text-[12px] font-mono px-2 py-0.5 rounded-lg">
              {platform}
            </span>
          )}
        </div>
      </div>
      <div className="h-px bg-border-light mb-4" />

      {buildInfo && (
        <div className="mb-4 border-y border-border-light py-3">
          <div className="grid grid-cols-2 gap-x-4 gap-y-2 text-[12px]">
            <span className="text-text-muted">{t("settings.about.upstreamBaseline")}</span>
            <span className="font-mono text-text-secondary text-right truncate">v{buildInfo.upstream_baseline}</span>
            <span className="text-text-muted">{t("settings.about.commit")}</span>
            <span className="font-mono text-text-secondary text-right truncate">{buildInfo.commit}</span>
            <span className="text-text-muted">{t("settings.about.channel")}</span>
            <span className="font-mono text-text-secondary text-right truncate">{buildInfo.channel}</span>
            <span className="text-text-muted">{t("settings.about.buildDate")}</span>
            <span className="font-mono text-text-secondary text-right truncate">{buildInfo.built_at}</span>
          </div>
          <button
            type="button"
            title={t("settings.about.copyDiagnostics")}
            onClick={copyDiagnostics}
            className="mt-3 h-8 w-full flex items-center justify-center gap-2 rounded-lg border border-border text-[12px] text-text-secondary hover:bg-bg-input cursor-pointer"
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {copied ? t("settings.about.copied") : t("settings.about.copyDiagnostics")}
          </button>
        </div>
      )}

      <p className="text-[11px] font-semibold text-text-muted tracking-[0.6px] mb-1">
        {t("settings.about.currentVersion").toUpperCase()}
      </p>
      <button
        onClick={() => open(buildInfo?.repository ?? CURRENT_REPOSITORY_URL)}
        className="group flex items-center justify-between h-[57px] cursor-pointer"
      >
        <div className="flex items-center gap-3">
          <Github size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.repository")}</span>
        </div>
        <ExternalLink size={14} className="text-text-muted" />
      </button>

      <button
        onClick={() => open(CURRENT_RELEASES_URL)}
        className="group flex items-center justify-between h-[57px] cursor-pointer"
      >
        <div className="flex items-center gap-3">
          <BookText size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.releases")}</span>
        </div>
        <ExternalLink size={14} className="text-text-muted" />
      </button>

      <button
        onClick={() => open(CURRENT_ISSUES_URL)}
        className="group flex items-center justify-between h-[57px] cursor-pointer"
      >
        <div className="flex items-center gap-3">
          <Bug size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.issues")}</span>
        </div>
        <ExternalLink size={14} className="text-text-muted" />
      </button>
      <button
        onClick={() => open(CURRENT_DOCS_URL)}
        className="group flex items-center justify-between h-[57px] cursor-pointer"
      >
        <div className="flex items-center gap-3">
          <BookText size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.documentation")}</span>
        </div>
        <ExternalLink size={14} className="text-text-muted" />
      </button>

      <div className="h-px bg-border-light my-3" />
      <p className="text-[11px] font-semibold text-text-muted tracking-[0.6px] mb-1">
        {t("settings.about.upstreamProject").toUpperCase()}
      </p>
      <button
        onClick={() => open(buildInfo?.upstream_repository ?? UPSTREAM_REPOSITORY_URL)}
        className="group flex items-center justify-between h-[57px] cursor-pointer"
      >
        <div className="flex items-center gap-3">
          <GitFork size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.originalRepository")}</span>
        </div>
        <ExternalLink size={14} className="text-text-muted" />
      </button>
      <div className="flex items-center justify-between h-[57px]">
        <div className="flex items-center gap-3">
          <Scale size={16} className="text-text-muted" />
          <span className="text-[14px] text-text-primary tracking-[-0.15px]">{t("settings.about.license")}</span>
        </div>
        <span className="text-[12px] text-text-muted">MIT · yicheng47/quill</span>
      </div>

      <div className="flex-1" />
      <div className="flex items-center justify-center text-[11px] text-text-muted">
        {t("settings.about.basedOn")}
      </div>
    </div>
  );
}
