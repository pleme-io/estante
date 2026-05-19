;; consumer's `.frostrc.lisp` — what frost reads at startup.
;;
;; Point $FROSTRC at this file to drive the demo:
;;   FROSTRC=examples/frostmourne-style/consumer/frostrc.lisp frost
;;
;; The defsource line is essential — it pulls the deflockedpkg entries
;; from shellpkg.lock.lisp into scope. Without that, the defload below
;; errors with `LispError::UnknownPkg`.

(defsource :path "./shellpkg.lock.lisp")

(defload :pkg "example-pkg")

;; You can also (defload …) further packages here, mix with user
;; forms (defalias, defhook, etc.), defsource other files. estante
;; just contributes the lockfile + packaged rc.lisps; everything else
;; is normal frost-lisp.

(defalias :name "estante-status" :value "estante info")
