# Known issues

Known problems in published releases. Each entry gives the tag and the commit it was built from, what is wrong, and where it is fixed.

## 0.2.4 (tag `app-v0.2.4` -> `8440df9a`)

- **verify-bundle reported 1 FAIL.** qrtz-app honored the `QRTZ_MODULE_HELPERS` environment variable (and `QRTZ_<ID>_BIN` / `QRTZ_HELPER_BIN`), which could let the environment choose which helper binaries run.
- **Status:** published as a one-time exception.
- **Fix:** fixed on main in `a11e420e`. Those variables are now honored only in dev-local builds, and verify-bundle refuses them. 0.2.5 is the first release with the fix.
