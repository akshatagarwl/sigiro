export const TOOLTIP_DELAY = { delayDuration: 100, delay: 100 };

/**
 * Radix: `disableHoverableContent`. Base UI: `disableHoverablePopup`.
 *
 * Both belong on the tooltip root. Radix also accepts its spelling on the
 * provider; Base UI does not, which is why this is separate from the delay.
 */
export const TOOLTIP_NOT_HOVERABLE = {
  disableHoverableContent: true,
  disableHoverablePopup: true,
};

/**
 * Radix: `openDelay` / `closeDelay`.
 *
 * Base UI's preview card — what its `hover-card` is built on — takes neither,
 * so these apply on Radix and Base UI uses its own defaults.
 */
export const HOVER_CARD_DELAY = { openDelay: 0, closeDelay: 0 };
