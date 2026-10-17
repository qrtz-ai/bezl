import { z } from 'zod';

export const ErrorCode = z.enum([
  'E_VALIDATION',
  'E_UNKNOWN_CHANNEL',
  'E_VERSION',
  'E_FORBIDDEN',
  'E_NOT_FOUND',
  'E_TIMEOUT',
  'E_UNAVAILABLE',
  'E_PROVIDER',
  'E_INTERNAL',
]);
export type ErrorCode = z.infer<typeof ErrorCode>;

export const AppErrorJSON = z.object({
  name: z.literal('AppError'),
  code: ErrorCode,
  message: z.string(),
  retryable: z.boolean(),
  details: z.unknown().optional(),
});
export type AppErrorJSON = z.infer<typeof AppErrorJSON>;

/** The only error type that crosses process boundaries. */
export class AppError extends Error {
  override readonly name = 'AppError';
  constructor(
    readonly code: ErrorCode,
    message: string,
    readonly options: {
      retryable?: boolean;
      details?: unknown;
      cause?: unknown;
    } = {},
  ) {
    super(message, { cause: options.cause });
  }
  get retryable(): boolean {
    return (
      this.options.retryable ??
      (this.code === 'E_TIMEOUT' || this.code === 'E_UNAVAILABLE')
    );
  }
  toJSON(): AppErrorJSON {
    return {
      name: 'AppError',
      code: this.code,
      message: this.message,
      retryable: this.retryable,
      ...(this.options.details === undefined
        ? {}
        : { details: this.options.details }),
    };
  }
  static fromJSON(j: AppErrorJSON): AppError {
    return new AppError(j.code, j.message, {
      retryable: j.retryable,
      details: j.details,
    });
  }
  /** Normalise anything thrown into an AppError without leaking internals. */
  static from(err: unknown): AppError {
    if (err instanceof AppError) return err;
    const message = err instanceof Error ? err.message : String(err);
    return new AppError('E_INTERNAL', message, { cause: err });
  }
}
