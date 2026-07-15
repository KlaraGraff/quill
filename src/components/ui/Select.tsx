import { useState, useRef, useEffect, useLayoutEffect, useCallback, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Check } from "lucide-react";

interface SelectOption {
  value: string;
  label: string;
  group?: string;
}

interface SelectProps {
  label?: string;
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
  className?: string;
  placeholder?: string;
}

const MENU_GAP = 4;
const OPTION_HEIGHT = 40;
const GROUP_HEIGHT = 24;
const VIEWPORT_MARGIN = 8;
const PORTALED_THEME_VARS = [
  "--color-bg-page",
  "--color-bg-surface",
  "--color-bg-muted",
  "--color-bg-input",
  "--color-text-primary",
  "--color-text-body",
  "--color-text-secondary",
  "--color-text-muted",
  "--color-text-placeholder",
  "--color-border",
  "--color-border-light",
  "--color-accent",
  "--color-accent-text",
  "--color-accent-bg",
] as const;

function inheritedThemeVars(element: Element): CSSProperties {
  const computed = getComputedStyle(element);
  return PORTALED_THEME_VARS.reduce<CSSProperties>((style, name) => {
    const value = computed.getPropertyValue(name).trim();
    if (value) Object.assign(style, { [name]: value });
    return style;
  }, {});
}

export default function Select({ label, value, onChange, options, className = "", placeholder = "" }: SelectProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuStyle, setMenuStyle] = useState<CSSProperties>();

  const selected = options.find((o) => o.value === value);
  const groupCount = new Set(options.map((option) => option.group).filter(Boolean)).size;

  const handleClickOutside = useCallback(
    (e: MouseEvent) => {
      const target = e.target as Node;
      if (ref.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    },
    [],
  );

  useEffect(() => {
    if (!open) return;
    const handleScroll = (e: Event) => {
      if (menuRef.current?.contains(e.target as Node)) return;
      setOpen(false);
    };
    const handleResize = () => setOpen(false);
    document.addEventListener("mousedown", handleClickOutside);
    window.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleResize);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      window.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("resize", handleResize);
    };
  }, [open, handleClickOutside]);

  // The menu is portaled to <body> so it can't be clipped by overflow
  // containers (settings modal scroll area, accordion animations).
  useLayoutEffect(() => {
    if (!open || !buttonRef.current) return;
    const rect = buttonRef.current.getBoundingClientRect();
    const menuHeight = options.length * OPTION_HEIGHT + groupCount * GROUP_HEIGHT + 2;
    const spaceBelow = window.innerHeight - rect.bottom - MENU_GAP - VIEWPORT_MARGIN;
    const spaceAbove = rect.top - MENU_GAP - VIEWPORT_MARGIN;
    const openUp = menuHeight > spaceBelow && spaceAbove > spaceBelow;
    setMenuStyle({
      ...inheritedThemeVars(buttonRef.current),
      left: rect.left,
      width: rect.width,
      maxHeight: Math.min(menuHeight, openUp ? spaceAbove : spaceBelow),
      ...(openUp
        ? { bottom: window.innerHeight - rect.top + MENU_GAP }
        : { top: rect.bottom + MENU_GAP }),
    });
  }, [groupCount, open, options.length]);

  return (
    <div className={`relative ${className}`} ref={ref}>
      {label && (
        <label className="block text-[14px] font-semibold text-text-primary mb-1.5">
          {label}
        </label>
      )}
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full h-9 bg-bg-input rounded-lg px-3 text-[13px] font-medium text-text-primary flex items-center justify-between cursor-pointer border border-transparent hover:border-border transition-colors"
      >
        <span className="min-w-0 truncate text-left">{selected?.label ?? placeholder}</span>
        <ChevronDown size={16} className={`shrink-0 text-text-muted transition-transform ${open ? "rotate-180" : ""}`} />
      </button>

      {open && menuStyle &&
        createPortal(
          <div
            ref={menuRef}
            style={menuStyle}
            // The menu lives under document.body, so without this, pressing an
            // option registers as an outside click for ancestor popovers
            // (e.g. ReaderSettings) and closes them before the option's
            // onClick fires.
            onMouseDown={(e) => e.stopPropagation()}
            className="fixed z-[70] bg-bg-surface border border-border rounded-xl shadow-popover overflow-y-auto"
          >
            {options.map((option, index) => {
              const isActive = option.value === value;
              const showGroup = option.group && option.group !== options[index - 1]?.group;
              return (
                <div key={option.value}>
                  {showGroup && (
                    <div className="flex h-6 items-end px-4 pb-1 text-[10px] font-medium text-text-muted">
                      {option.group}
                    </div>
                  )}
                  <button
                    type="button"
                    onClick={() => {
                      onChange(option.value);
                      setOpen(false);
                    }}
                    className={`flex h-10 w-full cursor-pointer items-center justify-between gap-3 px-4 text-[14px] transition-colors ${
                      isActive
                        ? "bg-accent-bg text-accent-text"
                        : "text-text-primary hover:bg-bg-input"
                    }`}
                  >
                    <span className="min-w-0 truncate text-left">{option.label}</span>
                    {isActive && <Check size={16} className="shrink-0 text-accent-text" />}
                  </button>
                </div>
              );
            })}
          </div>,
          document.body,
        )}
    </div>
  );
}
