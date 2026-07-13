import { useEffect, useId, useState } from "react";
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
}: {
  config: CardModuleConfig;
  card: CardKindConfig;
  labelKey: string;
  content?: LearningModuleContent;
  loading: boolean;
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
    <section className="px-4 py-3">
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
          {t(labelKey)}
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

export default function LearningCardModules({ card, kind, content, loading = false }: LearningCardModulesProps) {
  const definitions = new Map(MODULE_DEFINITIONS[kind].map((item) => [item.id, item]));

  return (
    <div className="divide-y divide-border/60" aria-busy={loading || undefined}>
      {card.modules.filter((module) => module.enabled).map((module) => {
        const definition = definitions.get(module.id);
        if (!definition) return null;
        return (
          <ModuleSection
            key={module.id}
            config={module}
            card={card}
            labelKey={definition.labelKey}
            content={content[module.id]}
            loading={loading}
          />
        );
      })}
    </div>
  );
}
