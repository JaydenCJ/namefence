# namefence examples

`demo.sh` builds a small throwaway tree containing one of every problem
class namefence knows about — a reserved device name, a Windows-illegal
character, a trailing space, a case twin, an NFC/NFD duplicate pair, a
cloud-reserved name, an invalid-UTF-8 name — then walks through the whole
CLI against it: `check` (text and JSON), platform targeting, `fix` as a dry
run, `fix --apply`, and the `stdin` mode fed from a fake `git ls-files`
listing. The fixture lives in a temp directory and is deleted on exit; your
own files are never touched.

```bash
bash examples/demo.sh
```

Ideas to steal for your own setup:

```bash
# Gate a repository in CI: lint exactly what git tracks, fail on errors.
git ls-files -z | namefence stdin -0 --fail-on error

# Pre-sync sweep of a Dropbox/Syncthing folder, machine-readable.
namefence check --targets cloud --format json ~/Sync

# See what a cleanup would do, then do it.
namefence fix ~/Sync
namefence fix --apply ~/Sync
```
