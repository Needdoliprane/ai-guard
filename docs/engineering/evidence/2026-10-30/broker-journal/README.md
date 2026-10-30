# Journal privé du broker — 30 octobre 2026

Composant Rust de persistance du broker, construit depuis `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. [Contrat](../../../BROKER_JOURNAL.md), [implémentation](implementation.md), [primitives filesystem](filesystem-implementation.md), [tests Luna](tests.md), [preuve SQLite](sqlite-tests.md). [Revue indépendante close](review.md), sans constat bloquant résiduel au périmètre annoncé. Ce lot n'active ni CLI/service, ni schéma3, ni droit de lancement. La pile QA n'est pas modifiée.

## Résultats constatés

| Vérification | Résultat et portée |
|---|---|
| Journal macOS | **PASS** : 10 fonctions d'intégration, None/manuel/fournisseur, rejeu/CAS, caps, liens, codec nullable et versions imbriquées ; [log](macos/integration.log). |
| Erreurs et processus macOS | **PASS** : 4 fonctions actives, dont un self-test du harnais ; deux helpers ignorés par le filtre et appelés explicitement par leurs parents. Neuf SIGKILL après opérations réelles, erreurs injectées sans ACK et une course de création de verrou ; [log](macos/unit.log). Les helpers ne sont pas des PASS produit supplémentaires. |
| SQLite macOS | **PASS** : 1 fonction, vrai Store et Journal dans le même PID ; hardlink main refusé pendant l'inventaire, locks observés avant/après par enfants F_GETLK et PRAGMA, octets lus par FD retenu, append/intégrité après retrait du seul lien de test ; [log](sqlite/final.log). |
| Canaris historiques | **PASS** : 26 fonctions, `control_fs`15, `credential_handoff`7, `provider_runner`4 ; [log](macos/canaries.log). Aucun fournisseur réel. |
| Formatage et analyse | **PASS** : [fmt global](macos/fmt.log), [Clippy strict all-targets](macos/clippy.log). |
| Release | **PASS** : [unique build](macos/release.log), 40,20s ; seuls les trois if devenus let-chains ont changé après les tests macOS. Les tests Linux utilisent ensuite ce delta final. |
| Linux ARM64 virtualisé | **PASS ciblé** : mêmes 4 + 10 + 1 fonctions, sans les canaris macOS ; [log](linux/container.log). Les rejeux Linux ne créent pas de nouveaux tests uniques. Le self-test et les helpers gardent la même distinction. |

Environnement macOS27.2/DarwinARM64, Rust/Cargo1.99.1 : [versions](macos/environment.txt). Les [111 empreintes source](source.sha256), couvrant Cargo, toolchain, `src` et `tests`, sont identiques [avant](macos/source-hashes-before-release.txt) et [après](macos/source-hashes-after-release.txt) le build. Manifeste SHA256 `cd9775b29d3fa75a67b08ddd395a081e980665c95369fcb2c84f5a326302d520`.

Le [rapport Linux](linux.md) donne versions, image, conteneur et commandes exactes. L'[inspection finale](linux/container-inspect-final.txt) borne les ressources configurées ; la consommation n'est pas mesurée et le build n'utilise pas ces limites runtime. La [source recopiée depuis le conteneur arrêté](linux/container-source-manifest-verification.txt) est conforme111/111 au même manifeste. Le [manifeste des fichiers de preuve](SHA256SUMS) exclut uniquement sa propre entrée. Les rapports/logs bruts sont copiés sans réécriture ; le contrôle whitespace porte sur code, documents de suivi et ce README.

Artefacts conservés hors dépôt dans `/tmp/ai-guard-journal-qualified.LtbWas`, fichiers600 :

- `agentsandbox` : `39d88ea07a6ec7de50288d726ce263647cbde20bd7fbfd3cce28483a8efd22a4`.
- `libagentsandbox.rlib` : `1419376473a47dcc2d02cdc18e2129e7524144f6c844844c43e493803536ab92`.
- `source.tar` : `bc613e718b23bcdaac49233861a4bf13e93352b804ef2ce8150e1096c6204e14`.

## Défauts et historique

Le [RED initial](history/absent-module-red.log) établit l'absence du composant, pas une vulnérabilité. Le [rejet d'un champ connu nullable](history/checkpoint-null-red.log) est un défaut comportemental réel du nouveau constructeur, corrigé avec `serde_ignored` et [contrôles d'opacité/EOF](history/checkpoint-null-green.log).

La revue a fait ajouter le refus des versions inconnues de mutations et snapshots. Sept variantes ont été reproduites avant correction ; le huitième cas d'exclusion et les positifs historiques ont été ajoutés ensuite. La course réelle de première ouverture rendait Io au lieu de Busy ; relecture unique sur AlreadyExists après prévalidation, sans retry des refus de sûreté. Les nouveaux RED versions/verrou sont rapportés dans le fil et [le compte rendu](tests.md), mais leurs sorties brutes n'ont pas été conservées en fichiers. Les premiers diagnostics fmt/Clippy ont également été écrasés par les logs finaux ; aucune reconstitution n'est présentée comme un original.

Les trois échecs mécaniques du test SQLite sont conservés : [UUID](sqlite/initial-fail.log), [dispatch enfant](sqlite/child-dispatch-fail.log), [fermeture de FD par le test lui-même](sqlite/descriptor-close-fail.log). Aucun changement applicatif n'a été nécessaire pour ces trois corrections du harnais.

Le premier [Dockerfile avec FROM ID brut](linux/build-initial-fail.log) a échoué lors de la résolution de métadonnées. La [construction corrigée](linux/build-pinned.log) utilise un tag local unique dont l'ID est contrôlé, sans pull et hors réseau. Le cache local conservé n'est pas une image de distribution publique qualifiée.

## Limites

Le contrat distingue données privées et autorité : scope fourni par le caller de confiance, aucune identité authentifiée ou permission attestée par le journal. Le raccord IPC, la fraîcheur des credentials, les consommateurs d'échéances, le gardien TTY, la reprise/fencing et le FAIL du crash CLI restent ouverts. Les scénarios complets de contextualisation managed au niveau Journal et de restauration du pointeur contre le plancher mémoire n'ont pas de test ciblé dans ce lot ; ils restent requis avant activation.

SIGKILL n'est pas une panne électrique. Le hook CurrentPublished suit le helper rename+fsync : l'intervalle entre ces syscalls est **NOT_RUN**. Les caps logiques ne sont pas des quotas physiques bytes/inodes. Pas de garantie hostile même UID/root/admin, de détection SQLite universelle, de test des alias SHM ou de résistance à la restauration après restart.

**NOT_RUN** : hôte Linux natif/x86_64, Podman, workers, moteur d'exécution, modèle/fournisseur, navigateur/E2E, capacité100, cloud/Kubernetes et chaîne de livraison. L'emploi de Docker pour le harnais Linux ne qualifie pas le runtime des workers. Aucun gate global clôturé, facturation désactivée. Les rapports bruts gardent leurs gels et statuts historiques ; les résultats finaux ci-dessus en précisent la portée.
