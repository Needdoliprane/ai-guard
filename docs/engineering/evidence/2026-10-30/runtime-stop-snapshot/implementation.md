# Arrêt lié à un instantané — lot runtime

Date : 30 octobre 2026. Périmètre exclusif : `src/runtime.rs`, `src/runtime/managed.rs`.

## Défaut reproduit avant modification

Luna a exécuté `cargo test --locked --test runtime_stop_snapshot -- --nocapture` : **FAIL attendu**, exit 101. Les cas `config_changed` et `logging_changed` observaient un `kill <CID>` malgré une divergence de configuration, puis `exited` au lieu de la rétention `running` demandée. Rapport : `/tmp/ai-guard-runtime-stop-snapshot-tests.md`. Log : `/tmp/ai-guard-runtime-stop-red.xe94YI/target-red-evidence.log`.

Sauvegarde privée conservée : `/tmp/ai-guard-runtime-stop-baseline.obEPFZ/`.

## Changement implémenté

- Nouveau port public `ManagedRuntime::stop_snapshot_owned(&OwnedContainerSnapshot) -> Result<ContainerObservation>` : liaison handle/autorité/CID/nom/version, preuve privée complète d'image et de configuration, comparaison de l'identité, de l'allocation, du redémarrage et du fingerprint avant tout effet.
- Port interne commun `stop_snapshot_target(&str, &ContainerObservation)` pour les chemins runtime qui détiennent déjà la cible et son observation. Une observation inaccessible, divergente ou d'état non interprétable produit une erreur avant `kill`.
- Une instance reconnue non active (`created`, `configured`, `initialized`, `exited`, `stopped`, `dead` avec `running=false`) est retournée sans `kill`. Cela ne constitue pas une preuve de mesure terminale.
- Pour une instance active interprétable et conforme, `kill_target` revérifie l'autorité moteur avant la commande. Une inspection finale doit prouver la même configuration et l'arrêt. Une réponse de commande ne remplace jamais cette inspection.
- `Runtime::execution_kill` et la garde d'annulation interactive conservent et utilisent leur baseline. Le fallback d'erreur `execute_owned` utilise la baseline de reprise ou l'instantané créé après validation complète ; sans baseline, il retient l'instance avec résultat d'arrêt inconnu et n'émet aucun `kill`.
- `stop_owned(handle)` reste disponible comme compatibilité historique sans baseline, explicitement distinct du nouveau port. Aucun autre appel de ce port n'est ajouté.
- Les six raccordements `src/runner/managed.rs` sont la propriété de l'orchestrateur et ont été réalisés séparément.

Les observations de reprise peuvent être déjà actives ou terminales. Aucune exigence artificielle de statut `Created` n'a été ajoutée au port. La version absente conserve sa sémantique historique v1 ; les versions 1/2/3 restent acceptées avec un fingerprint présent.

## Vérification et intégration

- Formatage ciblé exécuté : `rustfmt --edition 2024 --config skip_children=true src/runtime.rs src/runtime/managed.rs`.
- Aucun `cargo`, test fournisseur, moteur réel ou changement de service exécuté par cet agent. **GREEN, compilation et revue indépendante : en attente de Luna et de l'orchestrateur**, pas des succès annoncés.
- Luna possède exclusivement `tests/runtime_stop_snapshot.rs` et prépare les cas exact actif, inspection refusée, `Running` inconnu, allocation Created divergente, configuration divergente et logging divergent.

Sources du patch avant revue :

```text
b2a371c95892fa5c6c85e2271b2c8073325d5e7d3f292bb611dee406652f134d  src/runtime.rs
6bae8e0b6c0fd3a7d38b832c8fcd556ad5bde6c587ee067c19c29b6e5a719766  src/runtime/managed.rs
```

## Limites maintenues

Inspecter et arrêter ne constituent pas une opération atomique du moteur. L'administrateur de confiance du moteur peut encore modifier un conteneur entre les commandes. Les erreurs conservent l'instance et ne produisent ni relevé terminal, ni libération de quota, ni usage inventé. Ce lot ne livre pas le broker permanent, son scheduler, le gardien TTY survivant à SIGKILL, le fencing D5, le watchdog macOS, ni la qualification Docker/Podman réelle. Il ne modifie aucun service OS, compte cloud, credential ou donnée persistante.
