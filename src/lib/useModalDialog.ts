import { useEffect, useRef, type RefObject } from "react";

export function useModalDialog(
  open: boolean,
  initialFocus?: RefObject<HTMLElement | null>,
) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!open || !dialog) return;
    if (!dialog.open) dialog.showModal();
    initialFocus?.current?.focus();
    return () => {
      if (dialog.open) dialog.close();
    };
  }, [open, initialFocus]);

  return dialogRef;
}
