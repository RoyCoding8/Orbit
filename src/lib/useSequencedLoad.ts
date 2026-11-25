import { useCallback, useEffect, useRef } from "react";

export type SequencedRequest = {
  isCurrent: () => boolean;
};

export function useSequencedLoad() {
  const sequence = useRef(0);

  useEffect(() => () => {
    sequence.current += 1;
  }, []);

  const start = useCallback((): SequencedRequest => {
    const token = ++sequence.current;
    return {
      isCurrent: () => token === sequence.current,
    };
  }, []);

  const invalidate = useCallback(() => {
    sequence.current += 1;
  }, []);

  return { start, invalidate };
}
