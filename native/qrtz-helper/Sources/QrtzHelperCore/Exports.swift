// ADR 0146 M1.5: the module-agnostic wire types and helpers moved to QrtzShared (owner: shared), where every
// module app can import them. Re-exported so code importing QrtzHelperCore keeps compiling unchanged.
@_exported import QrtzShared
