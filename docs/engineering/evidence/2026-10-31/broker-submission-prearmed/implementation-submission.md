# Couture submission broker — GEL1

Base : 7098e204a91182229c1a67bc8017271cb3d20918, worktree broker-service.
Périmètre exclusif : src/runner.rs et src/runner/broker.rs. Aucun commit.

## Changements

- Nouveau module public runner::broker. PreparedSubmission opaque, sans Serde et avec Debug fixe, conserve l'intention initiale, le Directory réellement admis et le PrivateFile manuel ou fournisseur. Getters intent, binding_inputs, initial_checkpoint et config conformes au plan.
- prepare_broker_submission refuse avant IO UUID nil, approve_hash, mode interactif et contexte de littéraux de contrôle hors bornes (64 valeurs, 4096 octets chacune, total 256 KiB, aucune valeur vide). Les échecs après ces gardes sont opaques, sans causes parser ou chemins privés.
- choose_plan reste le wrapper legacy ; le corps commun reçoit explicitement Legacy ou Broker avec UUID imposée avant projection. Les approbations historiques et validate_controlling_terminal restent sur le chemin legacy. Le Directory ouvert initialement est conservé. La résolution fournisseur et les parseurs existants sont réutilisés.
- Le freeze fournisseur conserve le PrivateFile déjà lu après création/synchronisation du record. La branche Broker n'appelle pas control_secrets/read_token : elle utilise uniquement le contexte de contrôle fourni. Le wrapper legacy garde sa lecture historique et l'ordre des validations dans le freeze. Les credentials manuels sont analysés et contrôlés avant retour de la préparation.
- BrokerRunState::from_job reconstruit un RunPlan typé depuis l'intention et le checkpoint courant (V1 ou runner inclus dans V2). Vérifications pures des versions/demande/fournisseur ; approbation et managed conservés, credential_source absent. Aucun FS, Auto, nouvelle résolution, session écrite ou appel réseau.
- credentials_from_job impose la racine config de l'intention, valide les handles retenus et les hashes/noms du binding durable, puis utilise le Snapshot minimal existant. Le chemin .env/source original, providers.json et les credentials legacy ne sont jamais relus. BrokerCredentials expose path/names/secrets/validate, sans Serde, avec Debug fixe et erreurs opaques. validate utilise persistent_revalidate pour le binding durable et le snapshot retenus.

## Sources gelées

- src/runner.rs : ba72d730ee8d611196bf1130ab8bfc9bd56ad1d3b7f9ea5996b1221daa5efac2
- src/runner/broker.rs : 83bc1e6ab623bc6cc2b6314f0e0b32c4007b0c4c1bda02a0e693f297d1a8930b

## Vérifications constatées

PASS : rustfmt --edition 2024 --config skip_children=true src/runner.rs src/runner/broker.rs ; git diff --check (aucune sortie).
RED préalable confirmé par root : compilation RC101 E0432, module absent, /tmp/ai-guard-broker-submission-red/cargo-red.log.
NOT_RUN par cet agent : Cargo, compilation, tests, Clippy, runtime, moteurs, fournisseurs. Luna possède le cache et attend le gel des deux périmètres (runner et runtime).

## Limites

Cette couture ne décide, n'admet, ne lance et n'arme rien. Authentification et profil autorisé puis réserve précèdent la préparation côté caller ; accept durable doit précéder toute décision/effet. La reprise doit utiliser from_job et le binding durable, jamais préparer de nouveau. Les faits locaux typés d'annulation/approbation/échec avant admission et avant intention valide appartiennent au prochain raccord de service ; aucun Worker fictif ni fichier parallèle ajouté. Aucun service permanent ou test de mort CLI qualifié par ce lot. Aucun fichier source hors ownership modifié ; les modifications docs/tests préexistantes sont conservées.
