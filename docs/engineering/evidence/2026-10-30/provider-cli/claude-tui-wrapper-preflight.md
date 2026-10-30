# Préflight local du wrapper PTY V8 — 2026-10-30

**Portée : outil seulement.** Aucun processus n'a été lancé; aucun modèle, fournisseur, credential, Docker ou terminal n'a été touché.

Vérification pure en mémoire avec un marqueur artificiel `SAFE_OUTPUT_ONLY` et handle synthétique `session_id=314159`:

- `store()` a enregistré le handle immédiatement, avant tout formatage; `load()` l'a relu avec succès.
- Le compte est `output.length = 16` **unités de code UTF-16 JavaScript** (pas un compte d'octets).
- `typeof TextEncoder !== "undefined"` vaut `false` dans functions V8; aucun `TextEncoder` n'est donc nécessaire au wrapper.
- Le marqueur synthétique seul a été évalué; aucun contenu PTY ou transcript n'a été conservé ou affiché.

Ce contrôle est **PASS wrapper uniquement**, pas un test de runtime, de Docker, d'authentification ou de fournisseur. La tentative de qualification TUI reste FAIL et ne doit pas être reclassée.
