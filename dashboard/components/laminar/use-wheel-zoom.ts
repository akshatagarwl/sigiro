// Adapted from Laminar, Apache-2.0. See public/licenses/laminar-Apache-2.0.txt.
import { type RefObject, useEffect, useLayoutEffect, useRef } from "react";

export const MIN_ZOOM = 1;
export const MAX_ZOOM = 20;

export function useWheelZoom(
  scrollRef: RefObject<HTMLDivElement | null>,
  zoom: number,
  setZoom: (zoom: number) => void
) {
  const zoomRef = useRef(zoom);
  const setZoomRef = useRef(setZoom);
  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);
  useEffect(() => {
    setZoomRef.current = setZoom;
  }, [setZoom]);
  useLayoutEffect(() => {
    const container = scrollRef.current;
    if (!container) return;
    const handleWheel = (event: WheelEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      if (!event.deltaY) return;
      const current = zoomRef.current;
      const next = Math.max(
        MIN_ZOOM,
        Math.min(MAX_ZOOM, current + (event.deltaY < 0 ? 0.5 : -0.5))
      );
      if (current === next || !container.scrollWidth) return;
      const mouseX = event.clientX - container.getBoundingClientRect().left;
      const fraction = (container.scrollLeft + mouseX) / container.scrollWidth;
      const width = (container.scrollWidth * next) / current;
      zoomRef.current = next;
      setZoomRef.current(next);
      requestAnimationFrame(() => {
        container.scrollLeft = Math.max(
          0,
          Math.min(fraction * width - mouseX, width - container.clientWidth)
        );
      });
    };
    container.addEventListener("wheel", handleWheel, { passive: false });
    return () => container.removeEventListener("wheel", handleWheel);
  }, [scrollRef]);
}
