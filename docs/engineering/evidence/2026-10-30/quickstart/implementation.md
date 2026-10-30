# Lot quick start — 30 octobre 2026

## Changements

- `docs/quickstart.md` créé : outils requis, Rust pinné, configuration privée extérieure au projet, démarrage Docker/Podman et exception Desktop explicite, login viewer, commande offline strict, refus, profils/review, modification isolée, vérifications de l'original/export neuf, suivi/arrêt/reprise, réglages et données conservées, fournisseur non interactif et dépannage.
- `README.md` : entrée directe vers le quick start, nom produit ai-guard, tableau des neuf vues, recette et limites Codex alignées sur l'essai réel, qualification Claude limitée à version/help et absence TTY explicites.
- Aucun code applicatif, secret, service, volume ou fichier d'exploitation modifié. Le checkout est non suivi par Git ; aucun stage/commit n'a été effectué.

## Vérifications exécutées

**PASS — syntaxe d'aide et version constatées** avec `/tmp/ai-guard-sqlite-freeze.DezAml/agentsandbox` :

```text
agentsandbox --help
agentsandbox --version                         -> agentsandbox 0.1.0
agentsandbox init --help
agentsandbox doctor --help
agentsandbox build --help
agentsandbox up --help
agentsandbox down --help
agentsandbox run --help
agentsandbox worker-status --help
agentsandbox worker-stop --help
agentsandbox worker-resume --help
agentsandbox export-workspace --help
agentsandbox verify-audit --help
agentsandbox healthcheck --help
rustup toolchain install --help
```

Les aides ont toutes rendu le code 0. `doctor` et `build` n'ont pas `--config-dir`. `down` n'a pas `--allow-rootful-runtime`. L'export et les commandes worker réutilisent le runtime de la session et n'ont pas `--runtime`. Ces distinctions sont rendues explicites dans le guide.

**PASS — confrontation aux sources** :

- `rust-toolchain.toml` : 1.99.1 ; `images/control.Dockerfile`/`worker.Dockerfile` : Node24 fourni dans le build, dashboard statique/Rust et fournisseurs facultatifs.
- `src/operations.rs` : flags, images construites, `up --force-recreate`, attente de disponibilité, fichiers originaux/projections, conservation des clés et volumes.
- `src/main.rs` et Compose Docker/Podman : ports loopback4000/8787, base `/data/audit.sqlite`, quota local3/6000mCPU/3Gio/3600s.
- `src/runtime.rs` : UID/GID1000, 2CPU, 1Gio, 256PID, readonly/caps/NNP, réseau none et stdin métier fermé ; limites non présentées comme des mesures.
- `src/runner.rs` et `src/policy.rs` : timeout1–86400s, strict allowlist exacte et refus, review/hash, credentials extérieurs au projet, bridge+observe obligatoires.
- `policies/default.json` : `/usr/bin/id` autorisé ; `/bin/echo exemple-non-autorise` hors allowlist.
- `front/src/main.ts`/`api.ts`/`control-view.ts` : neuf vues, jeton viewer et autorisation côté serveur, export JSONL distinct de l'export des fichiers.
- Preuve privée Codex existante lue sans credential : `/tmp/ai-guard-codex-real-report.md` et fichiers `commands.txt`/`second-commands.txt`. Le guide n'expose aucun chemin privé QA. Les flags retenus sont ceux du second essai, y compris `--skip-git-repo-check`, `--ephemeral`, le modèle demandé et le choix explicite du sandbox interne.

**PASS — liens locaux nouveaux contrôlés** : chaque fichier cible du guide existe ; les titres d'ancrage de `docs/operations.md` (programme/cible, arrêt/reprise, agents cloud) et du nouveau guide ont été relus. Les liens README vers `docs/quickstart.md` et sa section fournisseur pointent vers ces fichiers/titres. Aucun lien distant ou propriétaire n'a été inventé.

## Limites et statuts

- **NOT_RUN** : aucun build, moteur/E2E, `up`, `run`, export, arrêt, healthcheck réel, appel modèle ou test frontend/Rust n'a été relancé pour ce lot documentaire. Les aides et la lecture de sources ne valent pas qualification bout en bout du guide.
- **NOT_RUN** : Claude authentification/appel modèle et toute interaction TTY. Le guide les distingue de la preuve version/help existante.
- **BLOCKED** : qualification production globale et matrice complète restent ouvertes dans les gates ; Kubernetes, relais worker et supervision autonome après mort du broker restent absents/non qualifiés.
- **NOT_APPLICABLE** : migration de données/code runtime pour ce lot (documentation seule).
- **À intégrer par root** : revue indépendante des commandes/liens par le sous-agent Luna ; archivage/lien de la preuve Codex/Claude dans la validation courante ; cohérence finale avec les mises à jour d'exploitation dont root est propriétaire.

## Revue Luna initiale et correction du delta

**FAIL — revue initiale, conservée** dans `/tmp/ai-guard-quickstart-review.md` : cinq défauts documentaires empêchaient l'intégration. Les aides et liens passaient, mais le README utilisait encore la configuration sous checkout, le prompt/nom vérifié Codex différait du second essai réel, l'exemple README explorait sans créer le fichier cité, la propagation du programme moteur personnalisé était ambiguë et plusieurs assignations de chemins n'étaient pas citées.

Corrections effectuées dans `README.md` et `docs/quickstart.md` uniquement :

1. Même recommandation `$HOME/.ai-guard` et `--config-dir` dans les commandes concernées ; le défaut `.agentsandbox` reste décrit comme pris en charge, avec exclusion physique de la copie, sans inventer un refus de toute configuration sous projet.
2. Prompt anglais exact du second essai Codex, timeout300 explicite, nom `result.txt` ; export propre à cette session et comparaison aux octets `Codex local smoke test completed.` sans saut de ligne. Vérification de l'absence du nouveau fichier dans l'original.
3. Recette README alignée sur la création et lien direct vers l'export/vérification. L'exploration Claude est explicitement un exemple non qualifié, sans effet fichier attendu.
4. `--runtime-program` documenté par invocation sur doctor/build/up/run/down/stop/resume/export ; liste des commandes qui ne l'acceptent pas, aucune configuration globale inventée. Adoption du programme modifié reliée à l'exploitation.
5. Assignations de chemin et UUID citées ; arguments de chemins sous variables citées dans README et quick start. Ancienne mention « Six vues » du tableau documentaire retirée.

**PASS — vérifications ciblées du delta, sans exécution du parcours** :

- Aides du binaire gelé reconsultées pour doctor, build, up, down, run, worker-stop, worker-resume, export-workspace : exit0, options conformes.
- Sections/ancres modifiées relues, liens locaux nouveaux existants.
- Tous les blocs `sh` de chaque document extraits par `awk`, puis passés séparément à `zsh -n` avec `pipefail` : chaque contrôle exit0.
- `printf '%s' 'Codex local smoke test completed.' | cmp -` contre le fichier déjà exporté du second essai réel : exit0. Lecture seule de l'artefact existant ; aucun nouvel appel modèle ni nouveau worker.

Gel documentaire prêt pour relecture du delta :

```text
SHA-256 README.md
ddce592c231d248215ee51b632b45f6359a58e06bc124739e7584f2563eefbd9
SHA-256 docs/quickstart.md
8afdf17c3dec76e046ff21a42b05ef815e8e23fdea7bec0526ae0b85d01ec75f
```

La seconde revue indépendante reste **NOT_RUN** au moment de ce gel. Aucun build, test de suite, E2E, changement de service, lecture de credential ou appel modèle n'a été effectué pour ces corrections.
