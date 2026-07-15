import { Check } from "lucide-react";
import { useEffect, useState } from "react";

interface ColorControlProps {
  color: string;
  opacity: number;
  presets: readonly string[];
  colorLabel: string;
  pickerLabel: string;
  hexLabel: string;
  opacityLabel: string;
  onChange: (value: { color: string; opacity: number }) => void;
  minOpacity?: number;
}

function normalizeHexColor(value: string, fallback: string) {
  const trimmed = value.trim();
  const normalized = (trimmed.startsWith("#") ? trimmed : `#${trimmed}`).toUpperCase();
  return /^#[0-9A-F]{6}$/.test(normalized) ? normalized : fallback;
}

export default function ColorControl({
  color,
  opacity,
  presets,
  colorLabel,
  pickerLabel,
  hexLabel,
  opacityLabel,
  onChange,
  minOpacity = 5,
}: ColorControlProps) {
  const [colorDraft, setColorDraft] = useState(color);

  useEffect(() => setColorDraft(color), [color]);

  const updateColor = (next: string) => onChange({ color: next.toUpperCase(), opacity });
  const commitColor = () => {
    const normalized = normalizeHexColor(colorDraft, color);
    setColorDraft(normalized);
    updateColor(normalized);
  };

  return (
    <div className="space-y-3">
      <div>
        <p className="mb-2 text-[11px] text-text-muted">{colorLabel}</p>
        <div className="flex flex-wrap items-center gap-2">
          {presets.map((preset) => (
            <button
              key={preset}
              type="button"
              aria-label={preset}
              title={preset}
              onClick={() => updateColor(preset)}
              className={`flex size-7 items-center justify-center rounded-full border border-black/10 ${color === preset ? "ring-2 ring-accent ring-offset-2 ring-offset-bg-surface" : ""}`}
              style={{ backgroundColor: preset }}
            >
              {color === preset && <Check size={13} className="text-white drop-shadow" />}
            </button>
          ))}
          <label className="relative size-7 shrink-0 overflow-hidden rounded-full border border-border" title={pickerLabel}>
            <input
              type="color"
              value={color}
              aria-label={pickerLabel}
              onChange={(event) => updateColor(event.target.value)}
              className="absolute -inset-2 size-12 cursor-pointer border-0 bg-transparent p-0"
            />
          </label>
          <input
            value={colorDraft}
            maxLength={7}
            aria-label={hexLabel}
            onChange={(event) => setColorDraft(event.target.value.toUpperCase())}
            onBlur={commitColor}
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              commitColor();
            }}
            className="h-8 w-[88px] rounded-md border border-border bg-bg-input px-2 font-mono text-[11px] uppercase text-text-primary outline-none focus:border-accent"
          />
        </div>
      </div>
      <div className="flex items-center gap-3">
        <span className="w-[72px] shrink-0 text-[11px] text-text-muted">{opacityLabel}</span>
        <input
          type="range"
          min={minOpacity}
          max={100}
          step={1}
          value={opacity}
          aria-label={opacityLabel}
          onChange={(event) => onChange({ color, opacity: Number(event.target.value) })}
          className="h-1 flex-1 cursor-pointer accent-accent"
        />
        <span className="w-10 text-right text-[11px] tabular-nums text-text-secondary">{opacity}%</span>
      </div>
    </div>
  );
}
