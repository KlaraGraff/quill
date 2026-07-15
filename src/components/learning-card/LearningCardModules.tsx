import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { getEffectiveDensity, MODULE_DEFINITIONS } from "./config";
import type {
  CardKindConfig,
  CardModuleConfig,
  ContentDensity,
  LearningContentItem,
  LearningModuleContent,
  LearningModuleId,
} from "./types";

interface LearningCardModulesProps {
  card: CardKindConfig;
  kind: "word" | "phrase" | "passage";
  content: Partial<Record<LearningModuleId, LearningModuleContent>>;
  loading?: boolean;
  highlightedModuleId?: string | null;
  animateChanges?: boolean;
}

interface RenderedModule {
  config: CardModuleConfig;
  labelKey: string;
  content?: LearningModuleContent;
  visible: boolean;
  animateEntrance: boolean;
}

function hasContent(content: LearningModuleContent | undefined) {
  return Boolean(
    content?.heading
    || content?.summary
    || content?.quote
    || content?.meta?.length
    || content?.details?.length
    || content?.items?.length,
  );
}

function Item({
  item,
  density,
  exampleCount,
}: {
  item: LearningContentItem;
  density: ContentDensity;
  exampleCount: number;
}) {
  return (
    <li className="break-words">
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="text-[12px] font-semibold text-text-primary">{item.title}</span>
        {item.meta && density !== "compact" && (
          <span className="text-[10px] text-text-muted">{item.meta.join(" · ")}</span>
        )}
      </div>
      {item.text && <p className="mt-0.5 text-[12px] leading-[1.55] text-text-secondary">{item.text}</p>}
      {density !== "compact" && item.examples?.slice(0, exampleCount).map((example, index) => (
        <div key={`${example.source}:${index}`} className="mt-1 border-l-2 border-border pl-2 text-[11px] leading-[1.5]">
          <p className="text-text-secondary">{example.source}</p>
          {example.target && <p className="text-text-muted">{example.target}</p>}
        </div>
      ))}
    </li>
  );
}

function ModuleContent({
  moduleId,
  content,
  density,
  card,
}: {
  moduleId: LearningModuleId;
  content: LearningModuleContent;
  density: ContentDensity;
  card: CardKindConfig;
}) {
  const meta = density === "compact" ? content.meta?.slice(0, 2) : content.meta;
  const details = density === "compact"
    ? []
    : density === "standard"
      ? content.details?.slice(0, 1)
      : content.details;
  const itemLimit = moduleId === "key_terms"
    ? card.keyTermCount
    : density === "compact"
      ? 3
      : undefined;
  const items = itemLimit === undefined ? content.items : content.items?.slice(0, itemLimit);

  return (
    <div className="break-words text-[12px] leading-[1.55]">
      {content.heading && <p className="mb-1 font-semibold text-text-primary">{content.heading}</p>}
      {meta && meta.length > 0 && (
        <p className="mb-1 flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-text-muted">
          {meta.map((value) => <span key={value}>{value}</span>)}
        </p>
      )}
      {content.summary && <p className="text-text-primary">{content.summary}</p>}
      {details && details.length > 0 && (
        <ul className="mt-1.5 list-disc space-y-1 pl-4 text-text-secondary">
          {details.map((detail) => <li key={detail}>{detail}</li>)}
        </ul>
      )}
      {items && items.length > 0 && (
        <ul className="mt-2 space-y-2">
          {items.map((item, index) => (
            <Item
              key={`${item.title}:${index}`}
              item={item}
              density={density}
              exampleCount={card.exampleCount}
            />
          ))}
        </ul>
      )}
      {content.quote && (
        <blockquote className="border-l-2 border-accent/40 pl-3 italic text-text-secondary">
          {content.quote}
        </blockquote>
      )}
    </div>
  );
}

function ModuleSection({
  config,
  card,
  labelKey,
  content,
  loading,
  highlighted,
}: {
  config: CardModuleConfig;
  card: CardKindConfig;
  labelKey: string;
  content?: LearningModuleContent;
  loading: boolean;
  highlighted: boolean;
}) {
  const { t } = useTranslation();
  const panelId = useId();
  const [expanded, setExpanded] = useState(config.defaultExpanded);

  useEffect(() => {
    setExpanded(config.defaultExpanded);
  }, [config.defaultExpanded]);

  if (!loading && !hasContent(content)) return null;
  const density = getEffectiveDensity(config, card);

  return (
    <section
      data-module-id={config.id}
      className={`px-4 py-3 transition-colors duration-300 ${highlighted ? "bg-accent-bg ring-2 ring-inset ring-accent/50" : ""}`}
    >
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={panelId}
        onClick={() => setExpanded((value) => !value)}
        className="flex min-h-6 w-full items-start gap-2 text-left"
      >
        {expanded
          ? <ChevronDown size={14} className="mt-0.5 shrink-0 text-text-muted" />
          : <ChevronRight size={14} className="mt-0.5 shrink-0 text-text-muted" />}
        <span className="min-w-0 flex-1 break-words text-[12px] font-semibold text-text-primary">
          {config.id.startsWith("custom_") ? labelKey : t(labelKey)}
        </span>
        <span className="shrink-0 text-[10px] text-text-muted">{t(`settings.tools.density.${density}`)}</span>
      </button>
      {expanded && (
        <div id={panelId} className="pt-2 pl-[22px]">
          {loading && !content ? (
            <div className="space-y-2" aria-hidden="true">
              <div className="h-3 w-4/5 animate-pulse rounded-sm bg-bg-input" />
              <div className="h-3 w-3/5 animate-pulse rounded-sm bg-bg-input" />
            </div>
          ) : content ? (
            <ModuleContent moduleId={config.id} content={content} density={density} card={card} />
          ) : null}
        </div>
      )}
    </section>
  );
}

function AnimatedModule({
  entry,
  card,
  loading,
  highlighted,
  onExited,
}: {
  entry: RenderedModule;
  card: CardKindConfig;
  loading: boolean;
  highlighted: boolean;
  onExited: () => void;
}) {
  const [shown, setShown] = useState(entry.visible && !entry.animateEntrance);

  useEffect(() => {
    if (!entry.visible) {
      setShown(false);
      const fallback = window.setTimeout(onExited, 300);
      return () => window.clearTimeout(fallback);
    }
    const frame = window.requestAnimationFrame(() => setShown(true));
    return () => window.cancelAnimationFrame(frame);
  }, [entry.visible, onExited]);

  return (
    <div
      data-module-exiting={entry.visible ? undefined : "true"}
      className={`grid transition-[grid-template-rows,opacity] duration-[240ms] ease-in-out ${
        shown ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0"
      }`}
      onTransitionEnd={(event) => {
        if (event.currentTarget === event.target && !entry.visible && !shown) onExited();
      }}
    >
      <div className="min-h-0 overflow-hidden">
        <ModuleSection
          config={entry.config}
          card={card}
          labelKey={entry.labelKey}
          content={entry.content}
          loading={loading}
          highlighted={highlighted}
        />
      </div>
    </div>
  );
}

export default function LearningCardModules({
  card,
  kind,
  content,
  loading = false,
  highlightedModuleId,
  animateChanges = false,
}: LearningCardModulesProps) {
  const currentModules = useMemo(() => {
    const definitions = new Map(MODULE_DEFINITIONS[kind].map((item) => [item.id, item]));
    const enabledModules = card.modules.filter((module) => module.enabled);
    const lastCompletedIndex = enabledModules.reduce(
      (last, module, index) => hasContent(content[module.id]) ? index : last,
      -1,
    );
    const visibleModules = loading
      ? enabledModules.slice(0, Math.min(enabledModules.length, Math.max(1, lastCompletedIndex + 2)))
      : enabledModules;
    return visibleModules.flatMap((module): RenderedModule[] => {
      const custom = module.id.startsWith("custom_") ? card.customModules[module.id as `custom_${string}`] : undefined;
      const definition = definitions.get(module.id) ?? (custom ? {
        id: module.id,
        labelKey: custom.name,
        descriptionKey: "",
        custom: true,
      } : undefined);
      return definition ? [{
        config: module,
        labelKey: definition.labelKey,
        content: content[module.id],
        visible: true,
        animateEntrance: false,
      }] : [];
    });
  }, [card, content, kind, loading]);
  const [animatedModules, setAnimatedModules] = useState<RenderedModule[]>(currentModules);
  const currentIdsRef = useRef(new Set(currentModules.map((entry) => entry.config.id)));

  useEffect(() => {
    currentIdsRef.current = new Set(currentModules.map((entry) => entry.config.id));
  }, [currentModules]);

  useEffect(() => {
    if (!animateChanges) return;
    setAnimatedModules((previous) => {
      const previousById = new Map(previous.map((entry) => [entry.config.id, entry]));
      const currentIds = new Set(currentModules.map((entry) => entry.config.id));
      const next = currentModules.map((entry) => ({
        ...entry,
        animateEntrance: !previousById.has(entry.config.id),
      }));
      for (const entry of previous) {
        if (!currentIds.has(entry.config.id)) next.push({ ...entry, visible: false });
      }
      return next;
    });
  }, [animateChanges, currentModules]);

  const renderedModules = animateChanges ? animatedModules : currentModules;

  return (
    <div className="divide-y divide-border/60" aria-busy={loading || undefined}>
      {renderedModules.map((entry) => animateChanges ? (
        <AnimatedModule
          key={entry.config.id}
          entry={entry}
          card={card}
          loading={loading}
          highlighted={highlightedModuleId === entry.config.id}
          onExited={() => {
            if (currentIdsRef.current.has(entry.config.id)) return;
            setAnimatedModules((current) => current.filter((item) => item.config.id !== entry.config.id));
          }}
        />
      ) : (
          <ModuleSection
            key={entry.config.id}
            config={entry.config}
            card={card}
            labelKey={entry.labelKey}
            content={entry.content}
            loading={loading}
            highlighted={highlightedModuleId === entry.config.id}
          />
      ))}
    </div>
  );
}
