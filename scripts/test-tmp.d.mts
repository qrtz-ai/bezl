export declare const RUN_PREFIX: string;
export declare const STALE_MS: number;
export declare function runTmpDir(
  env?: Record<string, string | undefined>,
): string;
export declare function sweepStale(
  parent: string,
  keep: string | undefined,
  now?: number,
): string[];
export default function setup(): () => void;
