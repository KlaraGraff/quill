import { useState, useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Globe, BookOpen, Bot, Wrench, Cloud, Info, Terminal, X, ChevronRight, Palette } from "lucide-react";
import GeneralSettings from "./settings/GeneralSettings";
import AppearanceSettings from "./settings/AppearanceSettings";
import ReadingSettings from "./settings/ReadingSettings";
import AiSettings from "./settings/AiSettings";
import ToolsSettings from "./settings/ToolsSettings";
import LibrarySyncSettings from "./settings/LibrarySyncSettings";
import McpSettings from "./settings/McpSettings";
import AboutSettings from "./settings/AboutSettings";
import Toast from "./ui/Toast";
import { useSettings } from "../hooks/useSettings";

export type SettingsSection = "general" | "appearance" | "reading" | "ai" | "tools" | "librarySync" | "mcp" | "about";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
  initialSection?: SettingsSection;
}

export default function SettingsModal({ open, onClose, initialSection = "general" }: SettingsModalProps) {
  const { t } = useTranslation();
  const [activeSection, setActiveSection] = useState<SettingsSection>(initialSection);
  const { settings, loading, save, saveBulk } = useSettings();
  const modalRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  // Toast state
  const [showToast, setShowToast] = useState(false);
  const [toastMessage, setToastMessage] = useState("");
  const toastTimeout = useRef<ReturnType<typeof setTimeout>>(undefined);
  const showSavedToast = (msg = t("settings.saved")) => {
    if (toastTimeout.current) clearTimeout(toastTimeout.current);
    setToastMessage(msg);
    setShowToast(true);
    toastTimeout.current = setTimeout(() => setShowToast(false), 1500);
  };

  useEffect(() => {
    if (open) setActiveSection(initialSection);
  }, [open, initialSection]);

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    const modal = modalRef.current;
    const focusableSelector = [
      "button:not([disabled])",
      "[href]",
      "input:not([disabled])",
      "select:not([disabled])",
      "textarea:not([disabled])",
      "[tabindex]:not([tabindex='-1'])",
    ].join(",");
    window.requestAnimationFrame(() => {
      modal?.querySelector<HTMLElement>(focusableSelector)?.focus();
    });
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key !== "Tab" || !modal) return;
      const focusable = Array.from(modal.querySelectorAll<HTMLElement>(focusableSelector))
        .filter((element) => !element.hasAttribute("disabled") && element.getClientRects().length > 0);
      if (focusable.length === 0) {
        e.preventDefault();
        modal.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handler);
    return () => {
      document.removeEventListener("keydown", handler);
      previousFocusRef.current?.focus();
      previousFocusRef.current = null;
    };
  }, [open, onClose]);

  // AI save state (must be before early return)
  const [aiDirty, setAiDirty] = useState(false);
  const aiSaveRef = useRef<(() => void) | null>(null);

  if (!open) return null;

  const isMacos = navigator.userAgent.includes("Macintosh");

  const allSections: { id: SettingsSection; label: string; subtitle: string; paneSubtitle?: string; icon: typeof Globe }[] = [
    { id: "general", label: t("settings.general.title"), subtitle: t("settings.general.subtitle"), icon: Globe },
    { id: "appearance", label: t("settings.appearance.title"), subtitle: t("settings.appearance.subtitle"), icon: Palette },
    { id: "reading", label: t("settings.reading.title"), subtitle: t("settings.reading.subtitle"), icon: BookOpen },
    { id: "ai", label: t("settings.ai.shortTitle"), subtitle: t("settings.ai.shortSubtitle"), icon: Bot },
    { id: "tools", label: t("settings.tools.title"), subtitle: t("settings.tools.subtitle"), paneSubtitle: t("settings.tools.paneSubtitle"), icon: Wrench },
    { id: "librarySync", label: t("settings.librarySync.title"), subtitle: t("settings.librarySync.subtitle"), icon: Cloud },
    { id: "mcp", label: t("settings.mcp.title"), subtitle: t("settings.mcp.subtitle"), icon: Terminal },
    { id: "about", label: t("settings.about.title"), subtitle: t("settings.about.subtitle"), icon: Info },
  ];

  const sections = isMacos ? allSections : allSections.filter((s) => s.id !== "librarySync");

  const settingsProps = { settings, loading, save, saveBulk, showSavedToast };

  const renderContent = (): ReactNode => {
    switch (activeSection) {
      case "general": return <GeneralSettings {...settingsProps} />;
      case "appearance": return <AppearanceSettings {...settingsProps} />;
      case "reading": return <ReadingSettings {...settingsProps} />;
      case "ai": return <AiSettings {...settingsProps} onDirtyChange={setAiDirty} onSaveRef={(fn) => { aiSaveRef.current = fn; }} />;
      case "tools": return <ToolsSettings {...settingsProps} />;
      case "librarySync": return <LibrarySyncSettings {...settingsProps} />;
      case "mcp": return <McpSettings {...settingsProps} />;
      case "about": return <AboutSettings />;
    }
  };

  const active = sections.find((s) => s.id === activeSection);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-label={t("settings.title")}
        tabIndex={-1}
        className="flex max-h-[760px] flex-col overflow-hidden rounded-lg border border-border bg-white shadow-[0px_25px_50px_-12px_rgba(0,0,0,0.25)] transition-[width] dark:bg-bg-surface sm:flex-row"
        style={{
          width: activeSection === "tools"
            ? "min(1400px, calc(100vw - 32px))"
            : "min(780px, calc(100vw - 32px))",
          height: "min(80dvh, 760px)",
          minHeight: "min(480px, calc(100dvh - 32px))",
        }}
      >
        {/* Sidebar */}
        <div className="max-h-[148px] shrink-0 overflow-y-auto border-b border-border bg-bg-muted sm:max-h-none sm:w-[220px] sm:border-b-0 sm:border-r">
          <p className="text-[13px] font-semibold text-text-primary px-4 pt-4 pb-2">
            {t("settings.title")}
          </p>
          <nav className="grid grid-cols-2 gap-0.5 px-2 pb-2 sm:flex sm:flex-col sm:pb-0">
            {sections.map((section) => {
              const Icon = section.icon;
              const isActive = activeSection === section.id;
              return (
                <button
                  key={section.id}
                  onClick={() => setActiveSection(section.id)}
                  className={`flex h-[44px] w-full cursor-pointer items-center gap-2 rounded-[6px] px-2 text-left transition-colors sm:h-[56px] sm:gap-3 sm:rounded-[8px] sm:px-3 ${
                    isActive ? "bg-accent-bg" : "hover:bg-bg-input"
                  }`}
                >
                  <Icon
                    size={16}
                    className={`shrink-0 ${isActive ? "text-accent-text" : "text-text-muted"}`}
                  />
                  <div className="flex-1 min-w-0">
                    <p className={`text-[14px] font-medium leading-[20px] tracking-[-0.15px] ${
                      isActive ? "text-accent-text" : "text-text-secondary"
                    }`}>
                      {section.label}
                    </p>
                    <p className={`hidden text-[11px] font-medium leading-[16px] tracking-[0.06px] truncate sm:block ${
                      isActive ? "text-accent-text/60" : "text-text-muted"
                    }`}>
                      {section.subtitle}
                    </p>
                  </div>
                  <ChevronRight
                    size={14}
                    className={`shrink-0 ${isActive ? "text-accent-text" : "text-text-muted/40"}`}
                  />
                </button>
              );
            })}
          </nav>
        </div>

        {/* Content */}
        <div className="flex-1 flex flex-col min-w-0">
          {/* Header actions */}
          <div className="flex items-center justify-end gap-2 pr-3 pt-3">
            {activeSection === "ai" && (
              <button
                onClick={() => aiSaveRef.current?.()}
                disabled={!aiDirty}
                className={`text-[13px] font-medium px-3 py-1 rounded-lg cursor-pointer transition-colors ${
                  aiDirty
                    ? "text-accent-text hover:bg-accent-bg"
                    : "text-text-muted/40 cursor-default"
                }`}
              >
                {t("common.save")}
              </button>
            )}
            <button
              onClick={onClose}
              aria-label={t("common.close")}
              title={t("common.close")}
              className="size-7 flex items-center justify-center rounded-[10px] hover:bg-bg-input cursor-pointer"
            >
              <X size={16} className="text-text-muted" />
            </button>
          </div>

          {/* Scrollable content */}
          <div
            className="flex-1 overflow-y-scroll px-6"
            style={{ scrollbarGutter: "stable" }}
          >
            {/* Pane header — title + subtitle, then a rule with room
                below it. Suppressed for About, which leads with its
                centered identity card. */}
            {activeSection !== "about" && (
              <div className="flex flex-col gap-1">
                <h3 className="text-[18px] font-semibold text-text-primary">
                  {active?.label}
                </h3>
                <p className="text-[13px] text-text-muted">
                  {active?.paneSubtitle ?? active?.subtitle}
                </p>
                <div className="mt-3 h-px bg-border-light mb-2" />
              </div>
            )}

            {renderContent()}
          </div>
        </div>
      </div>

      {/* Toast */}
      {showToast && (
        <Toast>{toastMessage}</Toast>
      )}
    </div>
  );
}
