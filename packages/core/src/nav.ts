/**
 * In-renderer navigation requests between shell-owned UI (palette), sub apps (Home) and the shell router.
 * Sub apps never import each other; they ask the shell to switch apps and optionally open a meeting.
 * A pending meeting is held until the target view mounts and takes it (the view loads lazily).
 */
export interface NavRequest {
  app: string;
  meetingId?: string;
}

type Listener = (r: NavRequest) => void;
const listeners = new Set<Listener>();
let pendingMeeting: string | undefined;

export function requestNavigate(r: NavRequest): void {
  pendingMeeting = r.meetingId;
  for (const l of listeners) l(r);
}

export function onNavigateRequest(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Returns (and clears) the meeting a navigation asked to open, if any. */
export function takePendingMeeting(): string | undefined {
  const id = pendingMeeting;
  pendingMeeting = undefined;
  return id;
}
