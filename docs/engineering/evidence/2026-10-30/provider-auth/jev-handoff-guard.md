# Refus des variables Jev dans le handoff worker

Périmètre : `src/runner.rs` uniquement. Le parseur du chemin manuel `--secret-env-file` refuse désormais `JEV_API_KEY` et `JEV_API_KEY_FILE` dès lecture du nom, avant création de snapshot. Le diagnostic est fixe et ne contient aucune valeur. Le chemin générique conserve la possibilité de transmettre explicitement les deux clés métier OpenAI/Anthropic ; la détection des collisions par valeur reste inchangée.

Régression unique paramétrée : `runner::tests::manual_credential_handoff_refuses_jev_names_before_snapshot`. Elle utilise exclusivement des canaris et des répertoires temporaires privés. Elle vérifie les deux noms Jev, le message fixe et l’absence du répertoire `credential-snapshots` pour chaque refus. Son cas positif conserve OpenAI+Anthropic et vérifie qu’une collision avec un credential de contrôle est toujours refusée.

Commande RED puis GREEN, exactement la même :

```sh
CARGO_INCREMENTAL=0 cargo test --locked --lib runner::tests::manual_credential_handoff_refuses_jev_names_before_snapshot -- --exact
```

- RED : exit101, 1 failed ; le parseur accepte le premier nom Jev avant correction.
- GREEN : exit0, 1 passed, 0 failed, 0 ignored, 85 filtered out. Les deux cas Jev sont refusés avant tout snapshot ; le cas métier et le contrôle des collisions passent.
- PASS : `rustfmt --edition 2024 --check src/runner.rs`, exit0.
- NOT_RUN : suite globale, HTTP, moteur, modèle et E2E ; inutiles pour ce delta local et non revendiqués.

Aucun `.env` réel ni secret de l’hôte ouvert, aucune image construite, aucun fichier hors périmètre produit modifié. Relecture indépendante du delta à effectuer par root.

Gel : SHA-256 de `src/runner.rs` = `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4`.
