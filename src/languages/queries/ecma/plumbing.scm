(import_statement) @plumbing
; `export { a } from "b"`, `export * from "b"` and `export { a }`, but not
; `export function`, `export const` or `export default`.
(export_statement !declaration !value) @plumbing
