import { DEFAULT_CUSTOM_THEME_COLOR } from "@/features/appearance/constants";

export interface RgbColor {
  r: number;
  g: number;
  b: number;
}

export interface HslColor {
  h: number;
  s: number;
  l: number;
}

export interface HsvColor {
  h: number;
  s: number;
  v: number;
}

interface CustomThemePalette {
  light: Record<string, string>;
  dark: Record<string, string>;
  swatches: readonly [string, string, string, string];
}

interface OklchColor {
  l: number;
  c: number;
  h: number;
}

const CUSTOM_THEME_PROPERTIES = [
  "--background",
  "--foreground",
  "--card",
  "--card-foreground",
  "--popover",
  "--popover-foreground",
  "--primary",
  "--primary-foreground",
  "--secondary",
  "--secondary-foreground",
  "--muted",
  "--muted-foreground",
  "--accent",
  "--accent-foreground",
  "--enabled-accent",
  "--border",
  "--input",
  "--ring",
  "--sidebar",
  "--scrollbar-thumb-hover",
] as const;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function clampChannel(value: number): number {
  return Math.round(clamp(value, 0, 255));
}

function clampPercent(value: number): number {
  return Math.round(clamp(value, 0, 100));
}

function hslCss(h: number, s: number, l: number, alpha?: number): string {
  const hue = Math.round(((h % 360) + 360) % 360);
  const saturation = clampPercent(s);
  const lightness = clampPercent(l);
  return alpha === undefined
    ? `hsl(${hue} ${saturation}% ${lightness}%)`
    : `hsl(${hue} ${saturation}% ${lightness}% / ${alpha})`;
}

function oklchCss(l: number, c: number, h: number, alpha?: number): string {
  const lightness = clamp(l, 0, 1).toFixed(3);
  const chroma = Math.max(0, c).toFixed(3);
  const hue = Math.round(((h % 360) + 360) % 360);
  return alpha === undefined
    ? `oklch(${lightness} ${chroma} ${hue})`
    : `oklch(${lightness} ${chroma} ${hue} / ${alpha})`;
}

export function normalizeHexColor(value: string): string | null {
  const trimmed = value.trim();
  const withoutHash = trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
  if (/^[0-9a-fA-F]{3}$/.test(withoutHash)) {
    return `#${withoutHash
      .split("")
      .map((part) => `${part}${part}`)
      .join("")
      .toUpperCase()}`;
  }
  if (/^[0-9a-fA-F]{6}$/.test(withoutHash)) {
    return `#${withoutHash.toUpperCase()}`;
  }
  return null;
}

export function hexToRgb(value: string): RgbColor | null {
  const normalized = normalizeHexColor(value);
  if (!normalized) return null;
  const hex = normalized.slice(1);
  return {
    r: Number.parseInt(hex.slice(0, 2), 16),
    g: Number.parseInt(hex.slice(2, 4), 16),
    b: Number.parseInt(hex.slice(4, 6), 16),
  };
}

export function rgbToHex(color: RgbColor): string {
  return `#${[color.r, color.g, color.b]
    .map((channel) => clampChannel(channel).toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase()}`;
}

export function rgbToHsl({ r, g, b }: RgbColor): HslColor {
  const red = clampChannel(r) / 255;
  const green = clampChannel(g) / 255;
  const blue = clampChannel(b) / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;
  const lightness = (max + min) / 2;

  if (delta === 0) {
    return { h: 0, s: 0, l: Math.round(lightness * 100) };
  }

  const saturation =
    delta / (1 - Math.abs(2 * lightness - 1));
  let hue: number;
  if (max === red) {
    hue = 60 * (((green - blue) / delta) % 6);
  } else if (max === green) {
    hue = 60 * ((blue - red) / delta + 2);
  } else {
    hue = 60 * ((red - green) / delta + 4);
  }

  return {
    h: Math.round((hue + 360) % 360),
    s: Math.round(saturation * 100),
    l: Math.round(lightness * 100),
  };
}

export function hslToRgb({ h, s, l }: HslColor): RgbColor {
  const hue = (((h % 360) + 360) % 360) / 360;
  const saturation = clamp(s, 0, 100) / 100;
  const lightness = clamp(l, 0, 100) / 100;

  if (saturation === 0) {
    const channel = clampChannel(lightness * 255);
    return { r: channel, g: channel, b: channel };
  }

  const q =
    lightness < 0.5
      ? lightness * (1 + saturation)
      : lightness + saturation - lightness * saturation;
  const p = 2 * lightness - q;

  const hueToChannel = (offset: number): number => {
    let t = hue + offset;
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };

  return {
    r: clampChannel(hueToChannel(1 / 3) * 255),
    g: clampChannel(hueToChannel(0) * 255),
    b: clampChannel(hueToChannel(-1 / 3) * 255),
  };
}

export function rgbToHsv({ r, g, b }: RgbColor): HsvColor {
  const red = clampChannel(r) / 255;
  const green = clampChannel(g) / 255;
  const blue = clampChannel(b) / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;

  let hue = 0;
  if (delta !== 0) {
    if (max === red) {
      hue = 60 * (((green - blue) / delta) % 6);
    } else if (max === green) {
      hue = 60 * ((blue - red) / delta + 2);
    } else {
      hue = 60 * ((red - green) / delta + 4);
    }
  }

  return {
    h: Math.round((hue + 360) % 360),
    s: max === 0 ? 0 : Math.round((delta / max) * 100),
    v: Math.round(max * 100),
  };
}

export function hsvToRgb({ h, s, v }: HsvColor): RgbColor {
  const hue = ((h % 360) + 360) % 360;
  const saturation = clamp(s, 0, 100) / 100;
  const value = clamp(v, 0, 100) / 100;
  const chroma = value * saturation;
  const x = chroma * (1 - Math.abs(((hue / 60) % 2) - 1));
  const m = value - chroma;

  let red = 0;
  let green = 0;
  let blue = 0;

  if (hue < 60) {
    red = chroma;
    green = x;
  } else if (hue < 120) {
    red = x;
    green = chroma;
  } else if (hue < 180) {
    green = chroma;
    blue = x;
  } else if (hue < 240) {
    green = x;
    blue = chroma;
  } else if (hue < 300) {
    red = x;
    blue = chroma;
  } else {
    red = chroma;
    blue = x;
  }

  return {
    r: clampChannel((red + m) * 255),
    g: clampChannel((green + m) * 255),
    b: clampChannel((blue + m) * 255),
  };
}

function srgbToLinear(value: number): number {
  const normalized = clampChannel(value) / 255;
  return normalized <= 0.04045
    ? normalized / 12.92
    : ((normalized + 0.055) / 1.055) ** 2.4;
}

function rgbToOklch({ r, g, b }: RgbColor): OklchColor {
  const red = srgbToLinear(r);
  const green = srgbToLinear(g);
  const blue = srgbToLinear(b);
  const long = 0.4122214708 * red + 0.5363325363 * green + 0.0514459929 * blue;
  const medium = 0.2119034982 * red + 0.6806995451 * green + 0.1073969566 * blue;
  const short = 0.0883024619 * red + 0.2817188376 * green + 0.6299787005 * blue;
  const longRoot = Math.cbrt(long);
  const mediumRoot = Math.cbrt(medium);
  const shortRoot = Math.cbrt(short);
  const lightness =
    0.2104542553 * longRoot +
    0.7936177850 * mediumRoot -
    0.0040720468 * shortRoot;
  const a =
    1.9779984951 * longRoot -
    2.4285922050 * mediumRoot +
    0.4505937099 * shortRoot;
  const labB =
    0.0259040371 * longRoot +
    0.7827717662 * mediumRoot -
    0.8086757660 * shortRoot;
  const chroma = Math.sqrt(a * a + labB * labB);
  const hue = ((Math.atan2(labB, a) * 180) / Math.PI + 360) % 360;

  return { l: clamp(lightness, 0, 1), c: chroma, h: hue };
}

function relativeLuminance({ r, g, b }: RgbColor): number {
  const channel = (value: number): number => {
    const normalized = clampChannel(value) / 255;
    return normalized <= 0.03928
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  };

  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function readableForeground(color: RgbColor, hue: number): string {
  return relativeLuminance(color) > 0.56
    ? hslCss(hue, 24, 14)
    : "hsl(0 0% 98%)";
}

function deriveCustomThemePalette(value: string): CustomThemePalette {
  const normalized = normalizeHexColor(value) ?? DEFAULT_CUSTOM_THEME_COLOR;
  const rgb = hexToRgb(normalized) ?? { r: 22, g: 184, b: 196 };
  const hsl = rgbToHsl(rgb);
  const source = rgbToOklch(rgb);
  const hue = source.c < 0.01 ? hsl.h : source.h;
  const primaryChroma = source.c < 0.012 ? 0 : clamp(source.c, 0.045, 0.18);
  const quietChroma = Math.min(primaryChroma * 0.18, 0.03);
  const backgroundChroma = Math.min(primaryChroma * 0.08, 0.014);
  const lightPrimaryLightness = clamp(source.l, 0.30, 0.68);
  const darkPrimaryLightness = clamp(source.l + 0.20, 0.56, 0.78);
  const lightBackgroundLightness = clamp(0.90 + source.l * 0.12, 0.89, 0.985);
  const darkBackgroundLightness = clamp(0.115 + source.l * 0.085, 0.115, 0.19);
  const lightPrimary = oklchCss(lightPrimaryLightness, primaryChroma, hue);
  const darkPrimary = oklchCss(darkPrimaryLightness, Math.max(primaryChroma * 0.72, source.c < 0.012 ? 0 : 0.05), hue);
  const lightAccent = oklchCss(clamp(lightBackgroundLightness - 0.03, 0.82, 0.955), Math.max(quietChroma, source.c < 0.012 ? 0 : 0.018), hue);
  const lightBackground = oklchCss(lightBackgroundLightness, backgroundChroma, hue);
  const darkBackground = oklchCss(darkBackgroundLightness, Math.min(primaryChroma * 0.12, 0.024), hue);

  return {
    light: {
      "--background": lightBackground,
      "--foreground": oklchCss(0.145, source.c < 0.012 ? 0 : 0.012, hue),
      "--card": oklchCss(clamp(lightBackgroundLightness + 0.018, 0.91, 1), backgroundChroma * 0.45, hue),
      "--card-foreground": oklchCss(0.145, source.c < 0.012 ? 0 : 0.012, hue),
      "--popover": oklchCss(clamp(lightBackgroundLightness + 0.018, 0.91, 1), backgroundChroma * 0.45, hue),
      "--popover-foreground": oklchCss(0.145, source.c < 0.012 ? 0 : 0.012, hue),
      "--primary": lightPrimary,
      "--primary-foreground": "oklch(0.985 0 0)",
      "--secondary": oklchCss(clamp(lightBackgroundLightness - 0.015, 0.82, 0.97), backgroundChroma * 0.6, hue),
      "--secondary-foreground": oklchCss(0.205, source.c < 0.012 ? 0 : 0.018, hue),
      "--muted": oklchCss(clamp(lightBackgroundLightness - 0.015, 0.82, 0.97), backgroundChroma * 0.6, hue),
      "--muted-foreground": oklchCss(0.556, source.c < 0.012 ? 0 : 0.018, hue),
      "--accent": lightAccent,
      "--accent-foreground": oklchCss(0.36, Math.min(primaryChroma * 0.72, 0.13), hue),
      "--enabled-accent": lightPrimary,
      "--border": oklchCss(clamp(lightBackgroundLightness - 0.062, 0.74, 0.925), backgroundChroma * 0.6, hue),
      "--input": oklchCss(clamp(lightBackgroundLightness - 0.062, 0.74, 0.925), backgroundChroma * 0.6, hue),
      "--ring": lightPrimary,
      "--sidebar": oklchCss(clamp(lightBackgroundLightness - 0.01, 0.84, 0.978), Math.min(primaryChroma * 0.08, 0.014), hue),
      "--scrollbar-thumb-hover": oklchCss(lightPrimaryLightness, primaryChroma, hue, 0.7),
    },
    dark: {
      "--background": darkBackground,
      "--foreground": oklchCss(0.94, source.c < 0.012 ? 0 : 0.006, hue),
      "--card": oklchCss(clamp(darkBackgroundLightness + 0.035, 0.16, 0.235), Math.min(primaryChroma * 0.11, 0.026), hue),
      "--card-foreground": oklchCss(0.94, source.c < 0.012 ? 0 : 0.006, hue),
      "--popover": oklchCss(clamp(darkBackgroundLightness + 0.05, 0.17, 0.25), Math.min(primaryChroma * 0.12, 0.028), hue),
      "--popover-foreground": oklchCss(0.94, source.c < 0.012 ? 0 : 0.006, hue),
      "--primary": darkPrimary,
      "--primary-foreground": oklchCss(0.16, source.c < 0.012 ? 0 : 0.025, hue),
      "--secondary": oklchCss(clamp(darkBackgroundLightness + 0.095, 0.22, 0.29), Math.min(primaryChroma * 0.10, 0.026), hue),
      "--secondary-foreground": oklchCss(0.92, source.c < 0.012 ? 0 : 0.006, hue),
      "--muted": oklchCss(clamp(darkBackgroundLightness + 0.085, 0.21, 0.28), Math.min(primaryChroma * 0.10, 0.024), hue),
      "--muted-foreground": oklchCss(0.70, source.c < 0.012 ? 0 : 0.012, hue),
      "--accent": oklchCss(clamp(darkBackgroundLightness + 0.12, 0.24, 0.33), Math.min(primaryChroma * 0.24, 0.055), hue),
      "--accent-foreground": oklchCss(0.88, Math.min(primaryChroma * 0.28, 0.055), hue),
      "--enabled-accent": darkPrimary,
      "--border": "rgb(255 255 255 / 10%)",
      "--input": "rgb(255 255 255 / 10%)",
      "--ring": darkPrimary,
      "--sidebar": oklchCss(clamp(darkBackgroundLightness + 0.02, 0.14, 0.21), Math.min(primaryChroma * 0.12, 0.026), hue),
      "--scrollbar-thumb-hover": oklchCss(darkPrimaryLightness, Math.max(primaryChroma * 0.72, source.c < 0.012 ? 0 : 0.05), hue, 0.72),
    },
    swatches: [
      lightPrimary,
      darkPrimary,
      lightAccent,
      lightBackground,
    ],
  };
}

export function getCustomThemeSwatches(value: string): readonly [string, string, string, string] {
  return deriveCustomThemePalette(value).swatches;
}

export function applyCustomThemeVariables(
  root: HTMLElement,
  value: string,
  dark: boolean,
): void {
  const palette = deriveCustomThemePalette(value);
  const variables = dark ? palette.dark : palette.light;
  CUSTOM_THEME_PROPERTIES.forEach((property) => {
    root.style.setProperty(property, variables[property]);
  });
}

export function clearCustomThemeVariables(root: HTMLElement): void {
  CUSTOM_THEME_PROPERTIES.forEach((property) => {
    root.style.removeProperty(property);
  });
}
