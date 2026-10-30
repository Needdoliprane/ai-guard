# Stratégie de qualification

Les résultats nouveaux sont dans [VALIDATION.md](VALIDATION.md). Les résultats historiques de [../validation.md](../validation.md) ne qualifient pas ce checkout sur macOS ARM64. Les critères détaillés existants sont dans [../acceptance.md](../acceptance.md) ; leur ancien découpage MVP/V2 ne réduit pas la mission production.

## Vérifications logicielles

Avec Rust `1.99.1`, fixé dans `rust-toolchain.toml` :

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked
cargo build --release --locked
```

Avec Node 24 dans `front/` :

```sh
npm ci
npm test
npm run build
npm run test:browser
```

Consigner les versions réellement utilisées. Le navigateur exige Playwright et Chromium. Un changement Rust de frontière de sécurité conserve un test négatif : identité fausse, lecture/écriture d'un autre tenant, panne d'audit, replay divergent ou substitution de chemin selon le périmètre.

Conformément à la préférence du mainteneur, confier l'exécution des tests aux agents Luna et sélectionner les cibles selon le changement ; ne pas relancer toute la matrice E2E à chaque lot. Les gates obligatoires restent à qualifier. Les [preuves de journalisation moteurv2](evidence/2026-10-30/engine-logs-v2/README.md) illustrent cette sélection : contrats/reprises ciblés, un scénario Docker503 et un run normal sur le candidat, sans nouveau navigateur puisque le frontend est inchangé. Le [vrai Codex batch](evidence/2026-10-30/provider-cli/README.md) garde son FAIL fonctionnel initial ; un exit0 fournisseur sans effet attendu n'est pas un PASS.

`api_auth_boundary` couvre l'ordre identité/rôle avant lecture des corps et extraction des paramètres, les deux alias, l'absence de mutation d'audit, la borne/ambiguïté Bearer et un vrai gateway enfant HTTP. Le client TCP annonce un corps de 1024 octets sans l'envoyer et exige un premier statut401 avant expiration du délai ; ce n'est pas un test de drainage ou de keep-alive. Le harnais Rust possède son child jusqu'à la fin (`kill_on_drop`), sans gateway de fond dépendant d'une session d'outil. Les [preuves RED/GREEN et régressions](evidence/2026-10-30/api-auth/README.md) restent distinctes d'une qualification de charge ou de runtime ; aucun moteur/modèle appelé.

`supervision_api` exerce les neuf routes v1 sur le vrai routeur et des bases SQLite privées A/B : guards avant body, protocole tenant/rôle/namespace, admission/armements/helpers/claims/export, liens au parent réel et maintenance après réouverture. Une sonde TCP loopback comptée vérifie le non-appel de Jev pour rejeu exact et admission fraîche inactive, après contrôle positif avec le vrai client ; aucun modèle ni contournement TLS. `broker_clock` vérifie calculs/UUID et échantillons natifs du macOS courant, dont un enfant Rust du même exécutable via pipe privé. Son entrée enfant ignorée est invoquée par le test parent et ne compte pas comme test produit supplémentaire. [Commandes, gels et limites](evidence/2026-10-30/supervision-api-clock/README.md) ; Linux natif, reboot/suspend et consommateurs d'échéances restent à qualifier séparément.

### Invocation et mises à jour du moteur

Les intégrations `runtime_managed`, `runtime_program` et `runner_recovery` couvrent le choix du programme hors PATH ambiant, l'environnement reconstruit, l'ancrage du répertoire de CLI, le refus des ACL non qualifiées et des fichiers non réguliers/surdimensionnés, ainsi que l'adoption explicite d'une ancienne invocation. Les tests de migration doivent garder le même moteur/worker/CID et les claims consommés, réessayer le même événement d'adoption après un refus d'audit et ne créer aucun événement supplémentaire pour une adoption déjà acquise. Le changement de contexte ou d'ID Docker entre diagnostic et effet est testé séparément pour les commandes opérateur.

Les programmes de substitution de ces tests sont des fixtures Rust natives explicitement sélectionnées, avec métadonnées privées ; ils ne qualifient ni Docker ni Podman. Pour les trois essais réels ci-dessous, `AGENTSANDBOX_TEST_RUNTIME_PROGRAM` permet de choisir explicitement le programme absolu de l'installation à qualifier. Son absence utilise les emplacements connus du produit ; le PATH du harness ne constitue pas une approbation. Consigner programme/politique réellement testés et conserver une preuve distincte pour la reprise d'une session antérieure.

## Qualification par runtime

Docker et Podman, Linux x86_64 et macOS ARM64 exigent quatre rapports distincts. Les trois essais batch ci-dessous et les cas moteurs interactifs sont ignorés par défaut ; leur absence d'exécution vaut `NOT_RUN`, jamais `PASS`.

| Cible Cargo | Prérequis et portée |
|---|---|
| `runtime_smoke` | Image worker et pile de contrôle démarrée, configuration et endpoints QA explicites ; exécution/refus, copie et export selon [../operations.md](../operations.md). |
| `runtime_confinement` | Image worker et moteur explicites ; configuration inspectée, effets de confinement ciblés, timeout, copie/export. Ne qualifie pas les contrats multi-client. |
| `runtime_terminal_persistence` | Image worker et moteur explicites ; crée sa configuration privée et un vrai gateway SQLite loopback. Un middleware refuse uniquement l'accusé terminal HTTP : vérifie conservation du conteneur, cohérence de l'observation, absence de faux terminal et intégrité après réouverture. Ce n'est pas une panne disque ni la réconciliation du produit. |
| `runtime_interactive` | Qualification ciblée Docker Desktop/macOS ARM64 avec image probe Rust : clavier/TTY/resize/exit7, doubleCtrlX et octets raw exacts CR/LF/Ctrl-C/UTF-8/flèche. PTY physique privé, driver maintenu jusqu'à vérification, aucune clé ni modèle. Trois cas réels PASS ; CtrlX rejoué une fois après revue avec motif opérateur durable exact, absence de timeout et délai propre <30s. Résultats, limites et FAIL préalables conservés dans le [dossier](evidence/2026-10-30/interactive-cli/README.md). Entrée `interactive_host_driver` interne ignorée, non comptée comme test produit ; `local_pty_host_driver_help_smoke` qualifie seulement le harnais sans moteur. |

Exemple de lancement ciblé (image déjà construite, aucun credential imprimé) :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_IMAGE=mon-image-worker:test \
cargo test --locked --test runtime_terminal_persistence -- --ignored --nocapture
```

Ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` uniquement pour une exception rootful explicitement choisie. Les répertoires de preuve, credentials QA privés, volumes et instances diagnostiques créés par le test terminal restent en place. Les fixtures CLI Rust des tests logiciels d'erreur ne qualifient aucun moteur.

La recette obligatoire à trois workers doit couvrir A1/A2 chez A et B1 chez B : démarrage/arrêt, données séparées, grant A1→A2, refus A1→B1, révocation, quotas, audit persistant, usage idempotent, redémarrage contrôle, arrêt brutal, dashboard et refus d'accès aux secrets/privilèges du contrôle. Ce scénario n'existe pas encore dans le code initial. La charge 100 workers est un essai séparé sur de vrais workers.

Kubernetes exige un adaptateur effectif, un cluster autorisé, un CNI appliquant les politiques et des preuves RBAC/identités/volumes/réseau/reprise. Aucune simulation ne qualifie cette plateforme. Les comptes fournisseurs et services distants exigent autorisation et versions réelles.
