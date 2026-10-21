import { useCallback, useState, type ReactNode } from 'react';
import { Button } from './Button';
import { Callout } from './Callout';

/**
 * Errors say what happened and what to do next (QRT-303, design principles: honest errors). The technical reason
 * never leads: it sits behind the callout's collapsed details.
 */
export interface UserError {
  /** What happened, lowercase: "couldn't save the dictionary". */
  title: string;
  /** The next step, or the reason when it is already plain language. */
  next: string;
  /** The raw reason (error codes, http statuses), shown only under details. */
  detail: string | null;
}

/**
 * Codes, statuses, identifiers, stack-like text, file paths (`/Users/…`, `~/…`, `\\server\…`, `C:\…`) and OS error
 * numbers (`os error 13`): fine for details, not for the main line.
 */
const TECHNICAL =
  /\b[hH][tT][tT][pP][sS]?\b|\b[A-Z][A-Z0-9_]{3,}\b|\b\d{3,}\b|\b(errno|exception|stack|null|undefined|NaN)\b|[{}<>[\]=]|::|\w\.\w+\(/;

/** An OS error number, as Rust prints it: `permission denied (os error 13)`. */
const OS_ERROR = /\bos error \d+\b/i;

/**
 * A path token: starts a word (or follows `:`, `,`, a quote or `(`) with `./`, `../`, `~/`, `~user/`, `/`, `\\` or a
 * drive letter, and continues with a path character. Bare relative names (`notes/db.sqlite`) and "and/or" stay
 * readable. `file:/` URLs are always technical.
 */
const PATH =
  /(?:^|[\s:,'"‘“(=])(?:\.{1,2}\/|~[\w-]*\/|\/|\\\\|[a-zA-Z]:\\)[\w.~$@%+-]|\bfile:\//i;

/** True when a message reads as plain language a person can act on. */
export function isReadable(msg: string): boolean {
  return (
    !!msg &&
    msg.length <= 140 &&
    !TECHNICAL.test(msg) &&
    !OS_ERROR.test(msg) &&
    !PATH.test(msg)
  );
}

export function userError(
  e: unknown,
  what: string,
  next = 'try again in a moment.',
): UserError {
  const raw =
    e instanceof Error
      ? e.message
      : typeof e === 'string'
        ? e
        : String(e ?? '');
  const readable = isReadable(raw);
  return {
    title: `couldn't ${what}`,
    next: readable ? raw : next,
    detail: readable ? null : raw || null,
  };
}

/** An error where the action happened: a danger Callout with the next step and the raw reason under details. */
export function ErrorCallout({
  error,
  action,
  onDismiss,
  'data-testid': testId,
}: {
  error: UserError | null;
  action?: ReactNode;
  onDismiss?: () => void;
  'data-testid'?: string;
}) {
  if (!error) return null;
  return (
    <Callout
      tone="danger"
      title={error.title}
      details={error.detail ?? undefined}
      action={
        action ??
        (onDismiss ? <DismissButton onClick={onDismiss} /> : undefined)
      }
      {...(testId ? { 'data-testid': testId } : {})}
    >
      {error.next}
    </Callout>
  );
}

function DismissButton({ onClick }: { onClick: () => void }) {
  return (
    <Button size="sm" variant="ghost" onClick={onClick}>
      dismiss
    </Button>
  );
}

/** The error a view shows inline, and a `fail(what)` that turns any rejection into it. */
export function useUserError() {
  const [error, setError] = useState<UserError | null>(null);
  const fail = useCallback(
    (what: string, next?: string) => (e: unknown) =>
      setError(userError(e, what, next)),
    [],
  );
  const clear = useCallback(() => setError(null), []);
  return { error, fail, clear };
}
