/* eslint-disable @typescript-eslint/no-explicit-any -- foliate-js has no TS definitions */
export type AnnotationStyleKind = "manual" | "automatic" | "vocab";

export interface FoliateView extends HTMLElement {
  open(file: string | File | Blob): Promise<void>;
  init(opts: { lastLocation?: string; showTextStart?: boolean }): Promise<void>;
  goTo(target: string | number): Promise<any>;
  prev(): Promise<void>;
  next(): Promise<void>;
  close(): void;
  book: any;
  renderer: any;
  lastLocation: any;
  history: {
    back(): void;
    forward(): void;
    canGoBack: boolean;
    canGoForward: boolean;
    addEventListener: EventTarget["addEventListener"];
    removeEventListener: EventTarget["removeEventListener"];
  };
  getCFI(index: number, range: Range): string;
  resolveCFI(cfi: string): { index: number; anchor: (doc: Document) => Range };
  getSectionFractions(): number[];
  addAnnotation(annotation: {
    value: string;
    color?: string;
    styleKind?: AnnotationStyleKind;
  }): Promise<any>;
  deleteAnnotation(annotation: { value: string }): Promise<void>;
  deselect(): void;
}
/* eslint-enable @typescript-eslint/no-explicit-any */

export interface TocChapter {
  title: string;
  href?: string;
  targetHref?: string;
  depth: number;
}

export interface ReaderPageInfo {
  current: number;
  visibleEnd?: number;
  total: number;
}

export interface ReaderNavigation {
  navigationId?: string;
  cfi?: string;
  openVocab?: boolean;
  openChat?: boolean;
  chatId?: string;
}
