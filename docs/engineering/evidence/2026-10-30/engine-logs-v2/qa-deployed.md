# Qualification runtime ciblée — 2026-10-30

## Résultat

- **PASS — run normal sans fournisseur** : session `41eb7bb4-8f9d-4d63-b654-6057aff35b34`, worker `9856309f-4e30-4759-a091-418d5836e2dc`, tentative `1de39df2-3eb6-463e-90c6-3fa61595ed6e`. Commande métier `/usr/bin/id`, profil `strict`, réseau `none`, provider `generic`, capture activée. Code de sortie 0, pas de timeout, post-traitement sans erreur, cleanup confirmé. La sortie capturée correspond à `uid=1000` (vérifiée par prédicat booléen, contenu complet non recopié).
- **PASS — admission V2 et état terminal** : statut observé `fixed_sandbox_v2`, terminal, quota non retenu, sortie capturée, cleanup confirmé. Image épinglée `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`, image ID `7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`.
- **PASS — compatibilité V1 en lecture** : l’ancienne session Codex `7c08e4e7-38b6-4bde-a978-376627c3bc6b` reste lisible avec le nouveau binaire, en V1 terminale, sans quota retenu et cleanup confirmé. Aucune reprise, export, mutation ou arrêt demandé.
- **PASS — audit** : avant le run, audit valide à 668 événements, tête `971b8923455befad82af3c0d1bad24c8d3b6a4d033e613ad1c2082c0d17fa021`. Après, audit valide à 690 événements, tête `40b460a20ea8bae0783893894d0a014205f1317b251ea8787a0f1755e6b5620b`.
- **PASS — original intact** : SHA-256 du README fixture avant/après identique : `4270d00d413082b6710f3b23fbf6bb34fb7ee669d67a6575c46e77152bf1721d`.
- **PASS — allocation usage** : la lecture bornée `api/usage?limit=100` contient exactement une allocation correspondant au worker et à la tentative, avec `billable=false`, `method_version=1`, runtime `docker`, provenance `observed`. Aucun contenu sensible n’a été affiché.

## Environnement et preuves

Binaire `/tmp/ai-guard-engine-logs-final.mGhUZD/agentsandbox`, SHA-256 `a4c2aab40b2b9184dac72301611286eb439fe9cedffffd231c4700db949b7c7a`. Docker CLI 29.2.1, moteur 29.9.2, socket local explicite. Le run utilisait la configuration QA existante et l’exception rootful autorisée. Les détails bruts et statuts restent dans les fichiers privés `/tmp/ai-guard-engine-qa-id-run.json`, `/tmp/ai-guard-engine-qa-id-run.stderr`, `/tmp/ai-guard-engine-qa-id-worker-status.json`, `/tmp/ai-guard-engine-old-codex-status.json`; aucun credential ni environnement complet n’est inclus dans ce rapport.

La pré/post vérification QA des 19 workers historiques V1 et du frontend est celle fournie par root (`qa-before-upgrade.json`, `qa-after-upgrade.json` dans le répertoire privé de preuves). Cette vérification ciblée ne qualifie ni Podman, ni d’autres plateformes, ni les contrôles transitifs.

## Vérification complémentaire de l’allocation

La source de `WorkerPageQuery::parse` dans `src/api.rs` impose `limit` entre 1 et 100 (défaut 50); le stockage applique aussi cette borne. Le premier essai `GET api/usage?limit=1000` a donc échoué HTTP 400 pour dépassement de limite. Une seule requête corrigée, `GET api/usage?limit=100`, a retourné 20 allocations. Filtrage local sur le worker et l’`attempt_id` de ce run : exactement une correspondance, `billable=false`, `method_version=1`, `runtime=docker`, provenance `observed`. Aucune pagination nécessaire. Les valeurs ont été résumées par un helper Rust temporaire hors dépôt ; seuls ces champs non secrets ont été affichés.
