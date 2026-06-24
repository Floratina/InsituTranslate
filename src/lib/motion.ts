import type { CSSProperties } from "react";

export const APP_MOTION_EASE = [0.03, 0.59, 0.19, 1] as const;
export const APP_CSS_EASE = `cubic-bezier(${APP_MOTION_EASE.join(", ")})`;

export const PRIMARY_PAGE_FADE_UP_MS = 300;
export const PRIMARY_PAGE_FADE_UP_DISTANCE_PX = 14;
export const SECONDARY_PAGE_FADE_UP_MS = 200;
export const SECONDARY_PAGE_FADE_UP_DISTANCE_PX = 8;

type FadeUpStyle = CSSProperties & {
  "--app-fade-up-duration": string;
  "--app-fade-up-ease": string;
  "--app-fade-up-y": string;
};

function createFadeUpStyle(durationMs: number, distancePx: number): FadeUpStyle {
  return {
    "--app-fade-up-duration": `${durationMs}ms`,
    "--app-fade-up-ease": APP_CSS_EASE,
    "--app-fade-up-y": `${distancePx}px`,
  };
}

export const PRIMARY_PAGE_FADE_UP_STYLE = createFadeUpStyle(
  PRIMARY_PAGE_FADE_UP_MS,
  PRIMARY_PAGE_FADE_UP_DISTANCE_PX,
);

export const SECONDARY_PAGE_FADE_UP_STYLE = createFadeUpStyle(
  SECONDARY_PAGE_FADE_UP_MS,
  SECONDARY_PAGE_FADE_UP_DISTANCE_PX,
);

export const PAGE_FADE_UP_TRANSITION = {
  duration: PRIMARY_PAGE_FADE_UP_MS / 1000,
  ease: APP_MOTION_EASE,
} as const;

export const SECONDARY_PAGE_FADE_UP_TRANSITION = {
  duration: SECONDARY_PAGE_FADE_UP_MS / 1000,
  ease: APP_MOTION_EASE,
} as const;
