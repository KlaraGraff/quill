import type { PageTurnAnimation } from "./ReaderSettings";

interface ViewTransitionHandle {
  finished: Promise<void>;
}

type TransitionDocument = Document & {
  startViewTransition?: (update: () => void | Promise<void>) => ViewTransitionHandle;
};

const pageTurnQueues = new WeakMap<HTMLElement, Promise<void>>();

type NativeSlideKind = "text" | "foliate" | null;

function nativeSlideKind(viewport: HTMLElement): NativeSlideKind {
  if (viewport.querySelector(".text-book-reader")) return "text";
  const view = viewport.querySelector("foliate-view") as (Element & {
    renderer?: Element | null;
  }) | null;
  return view?.renderer?.localName === "foliate-paginator" ? "foliate" : null;
}

export function supportsDocumentViewTransitions(doc: Document = document): boolean {
  return typeof (doc as TransitionDocument).startViewTransition === "function";
}

export function prefersReducedMotion(view: Window | null = window): boolean {
  return view?.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

async function runManualSlide(
  viewport: HTMLElement,
  direction: "previous" | "next",
  turn: () => void | Promise<void>,
): Promise<void> {
  const outClass = direction === "next"
    ? "reader-page-slide-out-next"
    : "reader-page-slide-out-previous";
  const inClass = direction === "next"
    ? "reader-page-slide-in-next"
    : "reader-page-slide-in-previous";
  viewport.classList.add(outClass);
  await wait(110);
  await turn();
  viewport.classList.remove(outClass);
  viewport.classList.add(inClass);
  await wait(170);
}

async function performPageTurnTransition({
  animation,
  direction,
  viewport,
  turn,
}: {
  animation: PageTurnAnimation;
  direction: "previous" | "next";
  viewport: HTMLElement | null;
  turn: () => void | Promise<void>;
}): Promise<void> {
  const reducedMotion = prefersReducedMotion(viewport?.ownerDocument.defaultView ?? window);
  if (!viewport || reducedMotion || animation === "none") {
    await turn();
    return;
  }

  // TXT smooth scrolling and Foliate's reflow paginator already animate slide
  // turns, including direct trackpad gestures that bypass this function. Keep
  // exactly one native animation; fixed-layout PDF uses the shared fallback.
  const nativeSlide = animation === "slide" ? nativeSlideKind(viewport) : null;
  if (nativeSlide) {
    await turn();
    if (nativeSlide === "text") {
      // Text scrollTo() is synchronous, so wait for smooth scrolling before a
      // queued rapid press computes its next spread.
      await wait(320);
    }
    return;
  }

  viewport.dataset.pageTurnActive = "true";
  const root = viewport.ownerDocument.documentElement;
  root.dataset.readerPageAnimation = animation;
  root.dataset.readerPageDirection = direction;
  const clear = () => {
    delete viewport.dataset.pageTurnActive;
    delete root.dataset.readerPageAnimation;
    delete root.dataset.readerPageDirection;
    viewport.classList.remove(
      "reader-page-fade-out",
      "reader-page-fade-in",
      "reader-page-cover-next",
      "reader-page-cover-previous",
      "reader-page-slide-out-next",
      "reader-page-slide-out-previous",
      "reader-page-slide-in-next",
      "reader-page-slide-in-previous",
    );
  };

  try {
    const transitionDocument = viewport.ownerDocument as TransitionDocument;
    if (transitionDocument.startViewTransition) {
      let updateRan = false;
      try {
        const transition = transitionDocument.startViewTransition(async () => {
          updateRan = true;
          await turn();
        });
        await transition.finished.catch(() => {});
      } catch {
        if (!updateRan) await turn();
      }
      return;
    }
    if (animation === "fade") {
      viewport.classList.add("reader-page-fade-out");
      await wait(120);
      await turn();
      viewport.classList.remove("reader-page-fade-out");
      viewport.classList.add("reader-page-fade-in");
      await wait(150);
      return;
    }
    // Without View Transitions, cover deliberately shares the slide fallback
    // so EPUB, fixed-layout PDF, and TXT all retain visible movement.
    await runManualSlide(viewport, direction, turn);
  } finally {
    clear();
  }
}

export function runPageTurnTransition(options: {
  animation: PageTurnAnimation;
  direction: "previous" | "next";
  viewport: HTMLElement | null;
  turn: () => void | Promise<void>;
}): Promise<void> {
  const { viewport } = options;
  if (!viewport) return performPageTurnTransition(options);

  const previous = pageTurnQueues.get(viewport) ?? Promise.resolve();
  const scheduled = previous
    .catch(() => {})
    .then(() => performPageTurnTransition(options));
  pageTurnQueues.set(viewport, scheduled);
  void scheduled.finally(() => {
    if (pageTurnQueues.get(viewport) === scheduled) pageTurnQueues.delete(viewport);
  }).catch(() => {});
  return scheduled;
}
