export type WheelTurnDirection = "previous" | "next";

export interface WheelPageTurnOptions {
  turn(direction: WheelTurnDirection): void;
  /** Return false to leave the event untouched, such as in scrolling mode. */
  isEnabled?(): boolean;
  /** Frames buffered per direction is stability * 2; halves are compared. */
  stability?: number;
  /** The newer half's average must clear this (px) to count as a push. */
  sensitivity?: number;
  /**
   * Cushion on the decay test. Above 1 steady scrolling still reads as a push
   * (Lethargy's default); below 1 only genuine acceleration does, which keeps a
   * sustained drag closer to a single page.
   */
  tolerance?: number;
  /** Repeated identical deltas inside this window are treated as inertia. */
  delayMs?: number;
  /** Lock after a turn, mirroring fullPage.js's transition lock. */
  cooldownMs?: number;
  now?(): number;
}

export interface WheelPageTurnHandler {
  handleWheel(event: WheelEvent): void;
  reset(): void;
}

const LINE_DELTA_PX = 16;
const PAGE_DELTA_PX = 360;

function normalizedDelta(event: WheelEvent): number {
  const dominant = Math.abs(event.deltaX) > Math.abs(event.deltaY)
    ? event.deltaX
    : event.deltaY;
  if (event.deltaMode === 1) return dominant * LINE_DELTA_PX;
  if (event.deltaMode === 2) return dominant * PAGE_DELTA_PX;
  return dominant;
}

function average(values: number[]): number {
  return values.reduce((total, value) => total + value, 0) / values.length;
}

/**
 * Page turning driven by Lethargy's inertia detector plus fullPage.js's
 * post-turn lock.
 *
 * Rather than guessing where one gesture ends, Lethargy buffers the recent
 * deltas and compares the average of the older half against the newer half.
 * Momentum can only decelerate, so a newer half that is not smaller means the
 * reader is actively pushing; a shrinking one is the inertia tail and is
 * ignored. A magnitude floor drops the tail's last few near-zero frames.
 *
 * Each direction keeps its own history, so reversing is never mistaken for the
 * previous direction decaying. After a turn the handler locks for cooldownMs,
 * which is what stops one sustained push from cascading into many pages.
 */
export function createWheelPageTurnHandler({
  turn,
  isEnabled,
  stability = 4,
  sensitivity = 5,
  tolerance = 1.1,
  delayMs = 150,
  cooldownMs = 250,
  now = () => Date.now(),
}: WheelPageTurnOptions): WheelPageTurnHandler {
  const size = stability * 2;
  const nextDeltas: (number | null)[] = new Array(size).fill(null);
  const previousDeltas: (number | null)[] = new Array(size).fill(null);
  const timestamps: number[] = new Array(size).fill(0);
  let lockedUntil = Number.NEGATIVE_INFINITY;

  const reset = () => {
    nextDeltas.fill(null);
    previousDeltas.fill(null);
    timestamps.fill(0);
    lockedUntil = Number.NEGATIVE_INFINITY;
  };

  const isDeliberate = (history: (number | null)[], timestamp: number): boolean => {
    // Warm-up bypass: until the window fills there is nothing to compare, so
    // Lethargy treats the event as deliberate. The post-turn cooldown keeps
    // that from cascading across the opening frames of the first swipe.
    if (history[0] === null) return true;
    // An unbroken run of identical deltas is synthetic rather than a real hand.
    if (
      timestamps[size - 2] + delayMs > timestamp
      && history[0] === history[size - 1]
    ) return false;
    const older = average(history.slice(0, stability) as number[]);
    const newer = average(history.slice(stability) as number[]);
    // Momentum can only decelerate: reject when the newer half is not larger
    // (the tail) or too small (its dying, near-zero frames).
    return Math.abs(older) < Math.abs(newer * tolerance)
      && Math.abs(newer) > sensitivity;
  };

  const handleWheel = (event: WheelEvent) => {
    if (event.ctrlKey) return;
    if (isEnabled && !isEnabled()) return;
    event.preventDefault();

    const delta = normalizedDelta(event);
    if (delta === 0) return;
    const timestamp = now();

    timestamps.push(timestamp);
    timestamps.shift();
    const history = delta > 0 ? nextDeltas : previousDeltas;
    history.push(delta);
    history.shift();

    if (timestamp < lockedUntil) return;
    if (!isDeliberate(history, timestamp)) return;
    lockedUntil = timestamp + cooldownMs;
    turn(delta > 0 ? "next" : "previous");
  };

  return { handleWheel, reset };
}
