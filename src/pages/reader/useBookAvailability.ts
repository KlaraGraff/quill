import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from "react";
import {
  checkBookAvailable,
  getBook,
  type Book,
  type BookAvailabilityStatus,
} from "../../hooks/useBooks";

export type ReaderAvailability = BookAvailabilityStatus | "checking" | "timeout" | "error";

export function useBookAvailability(
  book: Book | null,
  setBook: Dispatch<SetStateAction<Book | null>>,
) {
  const [availabilityState, setAvailabilityState] = useState<ReaderAvailability | null>(null);
  const [availabilityRetry, setAvailabilityRetry] = useState(0);

  useEffect(() => {
    if (!book || book.available !== false) {
      setAvailabilityState(null);
      return;
    }

    setAvailabilityState("checking");
    let cancelled = false;
    const startTime = Date.now();

    const poll = async () => {
      while (!cancelled) {
        if (Date.now() - startTime >= 60_000) {
          setAvailabilityState("timeout");
          return;
        }
        const result = await checkBookAvailable(book.id).catch(() => null);
        if (!result) {
          setAvailabilityState("error");
          return;
        }
        if (result.available) {
          const updated = await getBook(book.id).catch(() => null);
          if (updated?.available !== false) {
            setBook(updated);
            setAvailabilityState(null);
          } else {
            setAvailabilityState("error");
          }
          return;
        }
        if (result.status === "missing") {
          setAvailabilityState("missing");
          return;
        }
        setAvailabilityState("icloud_placeholder");
        await new Promise((resolve) => setTimeout(resolve, 2000));
      }
    };

    void poll();
    return () => { cancelled = true; };
  }, [book, availabilityRetry, setBook]);

  const retryAvailability = useCallback(() => {
    setAvailabilityRetry((value) => value + 1);
  }, []);

  return { availabilityState, retryAvailability };
}
