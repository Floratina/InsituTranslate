import { useMemo, useState, type PointerEvent } from "react";
import { RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DEFAULT_CUSTOM_THEME_COLOR } from "@/features/appearance/constants";
import {
  hexToRgb,
  hslToRgb,
  hsvToRgb,
  normalizeHexColor,
  rgbToHex,
  rgbToHsl,
  rgbToHsv,
  type HslColor,
  type HsvColor,
  type RgbColor,
} from "@/features/appearance/theme-colors";

interface CustomThemeColorPickerProps {
  value: string;
  onApply: (value: string) => void;
}

type RgbField = "r" | "g" | "b";
type HslField = "h" | "s" | "l";

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function rgbFromHex(value: string): RgbColor {
  return hexToRgb(value) ?? hexToRgb(DEFAULT_CUSTOM_THEME_COLOR)!;
}

function hsvFromRgb(rgb: RgbColor, hueFallback: number): HsvColor {
  const next = rgbToHsv(rgb);
  if (next.s === 0 || next.v === 0) {
    return { ...next, h: hueFallback };
  }
  return next;
}

function initialHsv(value: string): HsvColor {
  return rgbToHsv(rgbFromHex(normalizeHexColor(value) ?? DEFAULT_CUSTOM_THEME_COLOR));
}

function updateColorFromPointer(
  event: PointerEvent<HTMLDivElement>,
  onUpdate: (event: PointerEvent<HTMLDivElement>, rect: DOMRect) => void,
): void {
  event.currentTarget.setPointerCapture(event.pointerId);
  onUpdate(event, event.currentTarget.getBoundingClientRect());
}

function NumberField({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  return (
    <Label className="grid gap-1 text-xs text-muted-foreground">
      <span>{label}</span>
      <Input
        type="number"
        min={min}
        max={max}
        value={value}
        className="h-8"
        onChange={(event) => {
          const parsed = Number(event.target.value);
          if (!Number.isFinite(parsed)) return;
          onChange(clamp(parsed, min, max));
        }}
      />
    </Label>
  );
}

export function CustomThemeColorPicker({
  value,
  onApply,
}: CustomThemeColorPickerProps) {
  const initialValue = normalizeHexColor(value) ?? DEFAULT_CUSTOM_THEME_COLOR;
  const [draftHsv, setDraftHsv] = useState<HsvColor>(() => initialHsv(initialValue));
  const draftRgb = useMemo(() => hsvToRgb(draftHsv), [draftHsv]);
  const draftHsl = useMemo(() => rgbToHsl(draftRgb), [draftRgb]);
  const draftHex = useMemo(() => rgbToHex(draftRgb), [draftRgb]);
  const [hexDraft, setHexDraft] = useState(draftHex);
  const hexValid = normalizeHexColor(hexDraft) !== null;

  function commitHsv(nextHsv: HsvColor): void {
    const normalized = {
      h: clamp(nextHsv.h, 0, 360),
      s: clamp(nextHsv.s, 0, 100),
      v: clamp(nextHsv.v, 0, 100),
    };
    setDraftHsv(normalized);
    setHexDraft(rgbToHex(hsvToRgb(normalized)));
  }

  function commitRgb(nextRgb: RgbColor): void {
    const normalizedRgb = {
      r: Math.round(clamp(nextRgb.r, 0, 255)),
      g: Math.round(clamp(nextRgb.g, 0, 255)),
      b: Math.round(clamp(nextRgb.b, 0, 255)),
    };
    const nextHsv = hsvFromRgb(normalizedRgb, draftHsv.h);
    setDraftHsv(nextHsv);
    setHexDraft(rgbToHex(normalizedRgb));
  }

  function commitHsl(nextHsl: HslColor): void {
    const normalizedHsl = {
      h: clamp(nextHsl.h, 0, 360),
      s: clamp(nextHsl.s, 0, 100),
      l: clamp(nextHsl.l, 0, 100),
    };
    const nextRgb = hslToRgb(normalizedHsl);
    const nextHsv = hsvFromRgb(nextRgb, normalizedHsl.h);
    setDraftHsv(nextHsv);
    setHexDraft(rgbToHex(nextRgb));
  }

  function updateHex(nextValue: string): void {
    setHexDraft(nextValue);
    const normalized = normalizeHexColor(nextValue);
    if (!normalized) return;
    setDraftHsv((current) => hsvFromRgb(rgbFromHex(normalized), current.h));
  }

  function updateRgb(field: RgbField, nextValue: number): void {
    commitRgb({ ...draftRgb, [field]: nextValue });
  }

  function updateHsl(field: HslField, nextValue: number): void {
    commitHsl({ ...draftHsl, [field]: nextValue });
  }

  function resetDraft(): void {
    const defaultHsv = initialHsv(DEFAULT_CUSTOM_THEME_COLOR);
    setDraftHsv(defaultHsv);
    setHexDraft(DEFAULT_CUSTOM_THEME_COLOR.toUpperCase());
  }

  function applyDraft(): void {
    const normalized = normalizeHexColor(hexDraft);
    if (normalized) {
      onApply(normalized);
      return;
    }
    onApply(draftHex);
  }

  function pickSaturationValue(
    event: PointerEvent<HTMLDivElement>,
    rect: DOMRect,
  ): void {
    const saturation = clamp(((event.clientX - rect.left) / rect.width) * 100, 0, 100);
    const brightness = clamp(100 - ((event.clientY - rect.top) / rect.height) * 100, 0, 100);
    commitHsv({ h: draftHsv.h, s: saturation, v: brightness });
  }

  function pickHue(event: PointerEvent<HTMLDivElement>, rect: DOMRect): void {
    const hue = clamp(((event.clientX - rect.left) / rect.width) * 360, 0, 360);
    commitHsv({ ...draftHsv, h: hue });
  }

  return (
    <div className="grid gap-3">
      <div className="flex items-center gap-2">
        <span
          className="size-8 rounded-[6px] border shadow-sm"
          style={{ backgroundColor: draftHex }}
          aria-hidden="true"
        />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">自定义主题色</div>
          <div className="font-mono text-xs text-muted-foreground">{draftHex}</div>
        </div>
        <Button
          type="button"
          variant="outline"
          size="icon-sm"
          aria-label="恢复默认自定义主题色"
          onClick={resetDraft}
        >
          <RotateCcw className="size-3.5" />
        </Button>
      </div>

      <div
        role="slider"
        aria-label="选择饱和度和亮度"
        aria-valuetext={`S ${Math.round(draftHsv.s)}%, V ${Math.round(draftHsv.v)}%`}
        tabIndex={0}
        className="relative h-32 rounded-[6px] border outline-none focus-visible:ring-3 focus-visible:ring-ring/40"
        style={{
          background:
            `linear-gradient(to top, rgb(0 0 0), transparent), linear-gradient(to right, rgb(255 255 255), hsl(${draftHsv.h} 100% 50%))`,
        }}
        onPointerDown={(event) => updateColorFromPointer(event, pickSaturationValue)}
        onPointerMove={(event) => {
          if (event.buttons !== 1) return;
          pickSaturationValue(event, event.currentTarget.getBoundingClientRect());
        }}
      >
        <span
          className="pointer-events-none absolute size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_rgb(0_0_0_/_0.45)]"
          style={{
            left: `${draftHsv.s}%`,
            top: `${100 - draftHsv.v}%`,
          }}
          aria-hidden="true"
        />
      </div>

      <div
        role="slider"
        aria-label="选择色相"
        aria-valuenow={Math.round(draftHsv.h)}
        aria-valuemin={0}
        aria-valuemax={360}
        tabIndex={0}
        className="relative h-5 rounded-[6px] border outline-none focus-visible:ring-3 focus-visible:ring-ring/40"
        style={{
          background:
            "linear-gradient(to right, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
        }}
        onPointerDown={(event) => updateColorFromPointer(event, pickHue)}
        onPointerMove={(event) => {
          if (event.buttons !== 1) return;
          pickHue(event, event.currentTarget.getBoundingClientRect());
        }}
      >
        <span
          className="pointer-events-none absolute top-1/2 size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_rgb(0_0_0_/_0.45)]"
          style={{ left: `${(draftHsv.h / 360) * 100}%`, backgroundColor: draftHex }}
          aria-hidden="true"
        />
      </div>

      <Tabs defaultValue="hex" className="gap-2">
        <TabsList className="grid h-8 w-full grid-cols-3">
          <TabsTrigger value="hex">Hex</TabsTrigger>
          <TabsTrigger value="rgb">RGB</TabsTrigger>
          <TabsTrigger value="hsl">HSL</TabsTrigger>
        </TabsList>

        <TabsContent value="hex" className="grid gap-1.5">
          <Label className="text-xs text-muted-foreground" htmlFor="custom-theme-hex">
            Hex
          </Label>
          <Input
            id="custom-theme-hex"
            value={hexDraft}
            className="h-8 font-mono"
            spellCheck={false}
            aria-invalid={!hexValid}
            onChange={(event) => updateHex(event.target.value)}
          />
        </TabsContent>

        <TabsContent value="rgb" className="grid grid-cols-3 gap-2">
          <NumberField label="R" min={0} max={255} value={draftRgb.r} onChange={(next) => updateRgb("r", next)} />
          <NumberField label="G" min={0} max={255} value={draftRgb.g} onChange={(next) => updateRgb("g", next)} />
          <NumberField label="B" min={0} max={255} value={draftRgb.b} onChange={(next) => updateRgb("b", next)} />
        </TabsContent>

        <TabsContent value="hsl" className="grid grid-cols-3 gap-2">
          <NumberField label="H" min={0} max={360} value={draftHsl.h} onChange={(next) => updateHsl("h", next)} />
          <NumberField label="S" min={0} max={100} value={draftHsl.s} onChange={(next) => updateHsl("s", next)} />
          <NumberField label="L" min={0} max={100} value={draftHsl.l} onChange={(next) => updateHsl("l", next)} />
        </TabsContent>
      </Tabs>

      <div className="flex justify-end">
        <Button type="button" size="sm" disabled={!hexValid} onClick={applyDraft}>
          应用
        </Button>
      </div>
    </div>
  );
}
