// One-slide-per-gesture snap scrolling.
//
// The slide list is whatever `.slide` sections are in the board DOM, in order.
// Nothing here knows slide ids, so data-driven `slide:<slug>` slides drop in.
// Pure pieces (WheelGesture, stepIndex, easeInOutCubic) are unit-tested in
// tests/snap.test.ts without a DOM.

export type Step = -1 | 0 | 1;

/** Pixels a gesture must travel before it counts as one slide. */
export const WHEEL_THRESHOLD_PX = 40;
/** A pause this long between wheel events ends a gesture (trackpad inertia stays one gesture). */
export const GESTURE_QUIET_MS = 180;
export const SNAP_DURATION_MS = 420;

const LINE_PX = 16;
const PAGE_PX = 800;

/**
 * Turns a stream of wheel deltas into at most one step per gesture.
 * A gesture is a run of events with gaps < GESTURE_QUIET_MS. Once a gesture
 * has produced its step, the rest of it (inertia tail, extra notches) is
 * swallowed. While `locked` (animation running) nothing is produced.
 */
export class WheelGesture {
  private accumulated = 0;
  private last = Number.NEGATIVE_INFINITY;
  private spent = false;

  feed(deltaY: number, deltaMode: number, timeMs: number, locked: boolean): Step {
    const px = deltaMode === 1 ? deltaY * LINE_PX : deltaMode === 2 ? deltaY * PAGE_PX : deltaY;
    if (timeMs - this.last > GESTURE_QUIET_MS) {
      this.accumulated = 0;
      this.spent = false;
    }
    this.last = timeMs;
    if (locked) {
      // Input during the animation belongs to the gesture that started it.
      this.spent = true;
      return 0;
    }
    if (this.spent || !Number.isFinite(px) || px === 0) return 0;
    if (Math.sign(px) !== Math.sign(this.accumulated)) this.accumulated = 0;
    this.accumulated += px;
    if (Math.abs(this.accumulated) < WHEEL_THRESHOLD_PX) return 0;
    const step: Step = this.accumulated > 0 ? 1 : -1;
    this.accumulated = 0;
    this.spent = true;
    return step;
  }
}

export type SlideKey = "next" | "prev" | "home" | "end" | null;

/** Keys that move exactly one slide (or to the ends). */
export function slideKey(key: string): SlideKey {
  if (key === "PageDown" || key === "ArrowDown") return "next";
  if (key === "PageUp" || key === "ArrowUp") return "prev";
  if (key === "Home") return "home";
  if (key === "End") return "end";
  return null;
}

export function stepIndex(current: number, move: Exclude<SlideKey, null> | Step, total: number): number {
  if (total <= 0) return 0;
  const target =
    move === "home" ? 0 : move === "end" ? total - 1 : move === "next" ? current + 1 : move === "prev" ? current - 1 : current + move;
  return Math.max(0, Math.min(total - 1, target));
}

export function easeInOutCubic(t: number): number {
  const x = Math.max(0, Math.min(1, t));
  return x < 0.5 ? 4 * x * x * x : 1 - (-2 * x + 2) ** 3 / 2;
}

/** Animates board.scrollTop to a slide and holds an input lock until it lands. */
export class SnapAnimator {
  private frame = 0;
  private target = -1;
  private lockedUntil = 0;

  constructor(
    private readonly board: HTMLElement,
    private readonly onLand: (index: number) => void,
  ) {}

  get locked(): boolean {
    return this.frame !== 0 || performance.now() < this.lockedUntil;
  }

  get targetIndex(): number {
    return this.target;
  }

  go(slide: HTMLElement, index: number, animate: boolean): void {
    const board = this.board;
    const from = board.scrollTop;
    const to = from + (slide.getBoundingClientRect().top - board.getBoundingClientRect().top);
    this.cancel();
    this.target = index;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    if (!animate || reduced || Math.abs(to - from) < 1) {
      board.scrollTop = to;
      this.target = -1;
      this.onLand(index);
      return;
    }
    // Mandatory snapping would re-snap every intermediate scrollTop; pause it while animating.
    board.classList.add("is-snapping");
    const start = performance.now();
    const tick = (now: number) => {
      const t = (now - start) / SNAP_DURATION_MS;
      board.scrollTop = from + (to - from) * easeInOutCubic(t);
      if (t < 1) {
        this.frame = window.requestAnimationFrame(tick);
        return;
      }
      this.frame = 0;
      board.scrollTop = to;
      board.classList.remove("is-snapping");
      this.target = -1;
      // Short tail lock: a key repeat or inertia event right at landing must not chain.
      this.lockedUntil = performance.now() + 60;
      this.onLand(index);
    };
    this.frame = window.requestAnimationFrame(tick);
  }

  cancel(): void {
    if (this.frame) window.cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.board.classList.remove("is-snapping");
  }
}
