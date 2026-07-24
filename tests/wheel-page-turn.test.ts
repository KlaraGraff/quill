import assert from "node:assert/strict";
import test from "node:test";

import {
  createWheelPageTurnHandler,
  type WheelPageTurnOptions,
  type WheelTurnDirection,
} from "../src/components/wheel-page-turn.ts";

interface FakeWheelEventInit {
  deltaY?: number;
  deltaX?: number;
  deltaMode?: number;
  ctrlKey?: boolean;
}

function wheelEvent(init: FakeWheelEventInit): WheelEvent {
  return {
    deltaX: init.deltaX ?? 0,
    deltaY: init.deltaY ?? 0,
    deltaMode: init.deltaMode ?? 0,
    ctrlKey: init.ctrlKey ?? false,
    preventDefault() {},
  } as unknown as WheelEvent;
}

function harness(options: Partial<WheelPageTurnOptions> = {}) {
  const turns: WheelTurnDirection[] = [];
  let clock = 0;
  const handler = createWheelPageTurnHandler({
    turn: (direction) => turns.push(direction),
    now: () => clock,
    ...options,
  });
  return {
    turns,
    send(deltaY: number, advanceMs = 16, init: FakeWheelEventInit = {}) {
      clock += advanceMs;
      handler.handleWheel(wheelEvent({ deltaY, ...init }));
    },
  };
}

test("the first deliberate scroll turns a page immediately", () => {
  const { turns, send } = harness();
  send(40, 16);
  assert.deepEqual(turns, ["next"]);
});

test("the post-turn cooldown swallows the momentum tail", () => {
  const { turns, send } = harness();
  send(40, 16); // turns, then locks for cooldownMs
  for (const delta of [30, 22, 14, 8, 4, 2, 1]) send(delta, 20); // tail, all locked out
  assert.deepEqual(turns, ["next"]);
});

test("a fresh scroll after the cooldown turns again", () => {
  const { turns, send } = harness({ cooldownMs: 100 });
  send(40, 16);
  for (const delta of [30, 22, 14]) send(delta, 20);
  send(40, 200); // past cooldownMs since the turn
  assert.deepEqual(turns, ["next", "next"]);
});

test("upward scrolls turn to the previous page", () => {
  const { turns, send } = harness();
  send(-40, 16);
  assert.deepEqual(turns, ["previous"]);
});

test("each direction keeps its own history, so reversing turns the other way", () => {
  const { turns, send } = harness({ cooldownMs: 100 });
  send(40, 16);
  send(-40, 200); // past the cooldown, opposite direction
  assert.deepEqual(turns, ["next", "previous"]);
});

test("a single flick with an inertia tail turns exactly one page", () => {
  const { turns, send } = harness();
  // Accelerate, then decay: the cooldown after the first turn absorbs the rest.
  for (const delta of [20, 40, 55, 45, 30, 18, 10, 5, 2]) send(delta, 16);
  assert.deepEqual(turns, ["next"]);
});

test("a decaying inertia tail is rejected once the window is full", () => {
  // cooldownMs 0 isolates the Lethargy decision from the fullPage.js lock.
  const { turns, send } = harness({ cooldownMs: 0 });
  // A monotonic decay from the first frame: warm-up turns while the window
  // fills, then the newer half always averages below the older half.
  for (const delta of [70, 62, 54, 46, 38, 30, 22, 14]) send(delta, 30);
  const afterFill = turns.length;
  for (const delta of [10, 6, 4, 2]) send(delta, 30);
  assert.equal(turns.length, afterFill);
});

test("a sustained accelerating push keeps reading as deliberate", () => {
  const { turns, send } = harness({ cooldownMs: 0 });
  for (const delta of [10, 14, 18, 22, 26, 30, 34, 38]) send(delta, 30);
  const afterFill = turns.length;
  // Still speeding up past a full window, so it keeps turning.
  for (const delta of [42, 46, 50, 54]) send(delta, 30);
  assert.ok(turns.length > afterFill);
});

test("a long non-uniform drag turns a page per cooldown window", () => {
  // The documented Lethargy trade-off: a continuous deliberate drag reads as
  // deliberate the whole way (its speed never decays), so only the cooldown
  // rate-limits it. Deltas wobble so the stuck-value guard stays clear.
  const { turns, send } = harness({ cooldownMs: 250 });
  for (let i = 0; i < 60; i += 1) send(i % 2 === 0 ? 28 : 32, 20); // 1200ms
  assert.ok(turns.length > 1);
});

test("near-zero jitter never turns once the window is full", () => {
  const { turns, send } = harness({ cooldownMs: 0 });
  for (const delta of [70, 62, 54, 46, 38, 30, 22, 14]) send(delta, 30);
  const afterFill = turns.length;
  for (let i = 0; i < 8; i += 1) send(2, 30); // below sensitivity
  assert.equal(turns.length, afterFill);
});

test("dominant horizontal deltas are used and line mode is scaled", () => {
  const { turns, send } = harness();
  send(0, 16, { deltaX: 4, deltaMode: 1 });
  assert.deepEqual(turns, ["next"]);
});

test("ctrl+wheel (pinch zoom) is ignored", () => {
  const { turns, send } = harness();
  send(400, 16, { ctrlKey: true });
  assert.deepEqual(turns, []);
});

test("disabled handler ignores events", () => {
  const { turns, send } = harness({ isEnabled: () => false });
  send(400, 16);
  assert.deepEqual(turns, []);
});
