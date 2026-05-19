;; example-pkg :: rc.lisp
;; -----------------------------------------------------------------
;; Drop this into your shell config (via `defload`) and you get the
;; `example` alias. Real packages would also contribute hooks,
;; completions, prompts — anything frost-lisp's nine def-forms cover.

(defalias :name "example" :value "echo 'estante: hello from a defloaded package'")

(defhook :event "chpwd"
         :body  "echo 'estante: example-pkg saw a chpwd'")
