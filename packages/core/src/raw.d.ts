/** Vite/vitest `?raw` imports: the file's text. Used for the shared SQL migrations (db/sql). */
declare module '*.sql?raw' {
  const text: string;
  export default text;
}
