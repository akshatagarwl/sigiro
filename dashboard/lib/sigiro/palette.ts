/**
 * EvilCharts takes literal colours per theme (its ECharts charts draw on a
 * canvas, which cannot resolve `var()`), so these are the hex values of the
 * theme tokens in app/globals.css: the neutral `--chart-1..5` ramp and the
 * data-table block's `--error`/`--warning`/`--info`/`--success`. Keep them in
 * step with that file.
 */
export const TOKENS = {
  chart1: "#d4d4d4",
  chart2: "#737373",
  chart3: "#525252",
  chart4: "#404040",
  chart5: "#262626",
  error: "#ef4444",
  warning: "#f97315",
  info: "#0ea5e9",
  success: "#20c55e",
} as const;

const both = (color: string) => ({ light: [color], dark: [color] });

export const LEVEL_CHART = {
  error: { label: "Error", colors: both(TOKENS.error) },
  warning: { label: "Warning", colors: both(TOKENS.warning) },
  info: { label: "Info", colors: both(TOKENS.info) },
  debug: {
    label: "Debug",
    colors: { light: [TOKENS.chart1], dark: [TOKENS.chart4] },
  },
};

export const STATUS_CHART = {
  ok: {
    label: "OK",
    colors: { light: [TOKENS.chart2], dark: [TOKENS.chart3] },
  },
  error: { label: "Error", colors: both(TOKENS.error) },
};
