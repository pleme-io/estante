;; consumer's manifest. Authored by hand or via `estante add`.
;;
;; The `:source "local:…"` is what makes this demo offline. A real
;; entry would read `:source "github:owner/repo@v1.2.3"`.

(defshellpkg
  :name    "example-pkg"
  :version "0.1.0"
  :source  "local:../packages/example-pkg"
  :exports ("alias" "hook")
  )
