import { updateReadingProgress } from "../../hooks/useBooks";

export class ReadingProgressWriter {
  private pending: { bookId: string; progress: number; cfi: string } | null = null;
  private timer: number | null = null;
  private inFlight = false;

  queue(bookId: string, progress: number, cfi: string): void {
    this.pending = { bookId, progress, cfi };
    if (this.timer !== null || this.inFlight) return;
    this.schedule(750);
  }

  private schedule(delay: number): void {
    this.timer = window.setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, delay);
  }

  async flush(): Promise<void> {
    if (this.timer !== null) {
      window.clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.inFlight) return;
    const pending = this.pending;
    if (!pending) return;
    this.pending = null;
    this.inFlight = true;
    try {
      await updateReadingProgress(pending.bookId, pending.progress, pending.cfi);
    } catch {
      // A newer position is more useful than retrying an older failed write.
    } finally {
      this.inFlight = false;
      if (this.pending) this.schedule(250);
    }
  }
}
