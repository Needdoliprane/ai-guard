# Stockage SQLite privé — gel logiciel327 et QA, 30 octobre 2026

**PASS pour ce lot logiciel et les essais nommés ; produit non qualifié pour la production.** Les19candidats historiques de l'audit figé conservent leurs verdicts. Le périmètre vise les permissions, noms, descripteurs et sauvegardes SQLite, sans changer les schémas, DTO, tenants ou claims.

## Changement et régression

`StoreStorage` ancre le parent privé700 et sa chaîne ; main/WAL/SHM/journal présents doivent être réguliers, lien unique, UID effectif et600, avec contrôle d'ACL. Une initialisation sérialisée par verrou distinct par répertoire est bornée à5secondes. Les noms de main sont ASCII, les suffixes SQLite et le préfixe du verrou sont réservés sans distinction de casse ASCII ; les parents Unicode restent admis. Aucun fichier existant n'est réparé, renommé ou supprimé automatiquement.

L'ancien Store conservait un FD brut du main en parallèle de SQLite. Une copie isolée du gel302 a reproduit la perte de verrous POSIX d'une autre connexion du même processus après fermeture de ce FD : un enfant `F_GETLK` renvoie `F_UNLCK`. Le correctif laisse SQLite posséder ses FD main/SHM ; métadonnées par nom et contrôle VFS `HAS_MOVED` protègent les vérifications. Une violation est latchée. La même régression passe après correction, drop et ouverture refusée ; `PRAGMA journal_mode=DELETE` reste refusée pendant la connexion vivante. Ce défaut logiciel reproduit ne prouve pas une corruption de données ou une vulnérabilité formelle de l'instantané audité.

Le consumer de migration utilise le garde commun, une destination neuve privée et la synchronisation après fermeture. Les fixtures positives créent leurs répertoires700 et bases600 dès l'origine ; aucune assertion de sûreté n'a été supprimée pour obtenir un succès. [Core](core.md), [backup](backup.md), [revue indépendante](review.md).

## Vérifications

| Statut | Commande/scénario | Résultat observé |
|---|---|---|
| PASS | `cargo fmt --all -- --check` | Sans écart, rejoué après dernière fixture |
| PASS | `cargo clippy --locked --all-targets -- -D warnings` | Sans avertissement, rejoué après dernière fixture |
| PASS | `CARGO_INCREMENTAL=0 cargo test --locked` |327tests top-level,0FAIL,3ignorés. Les3résumés des enfants F_GETLK ne sont pas recomptés :330bruts ne signifie pas330tests indépendants |
| PASS | `cargo build --release --locked` | Binaire hôte gelé,24,98s |
| FAIL attendu puis PASS | Régressions permissions/backup, FD POSIX et noms Unicode réservés | Logs RED/GREEN conservés ; aucune modification de l'archive source initiale |
| PASS | Linux ARM64/overlayfs, UID1000, réseau none et sans montage |41intégrations+1unité VFS+1ACL POSIX réelle. Deux tests macOS exclus par `cfg`, pas comptés PASS Linux ; [rapport et limites](linux/README.md) |
| PASS | Deux UID Linux réels | Store1000 vivant avec main/WAL/SHM600 sous parent700 ; témoin public lisible par1001, lectures privées/rename/précréation refusés ; métadonnées inchangées. Sonde distincte, pas un test de confinement worker |
| PASS réel | `cargo test --release --locked --test runtime_confinement --test runtime_terminal_persistence -- --ignored --nocapture` avec Docker et image explicitement choisis | Confinement15,37s ; refus ACK terminal HTTP5039,34s avec instance arrêtée inspectable, audit valide et quota inconnu conservé ; [journal](ai-guard-sqlite-runtime-real.log) |
| PASS | Build complet contrôle+front dans Docker | Image nommée ci-dessous, source gelée, [journal](control-image-build.log) |
| PASS | Maintenance réelle du volume QA et redémarrage | Permissions modifiées explicitement hors ligne, hashes des deux bases inchangés, chaîne591 identique après nouveau backend ; voir ci-dessous |

Les tests ignorés ne sont pas des PASS de la commande globale. Deux ont été exécutés explicitement dans ce lot. Le smoke générique n'a pas été relancé systématiquement ; la qualification fournisseur réelle demandée ensuite est un lot séparé. Aucune matrice E2E entière n'a été répétée pour le seul changement visuel.

Rust1.99.1, macOS27.2 ARM64, Docker Desktop4.97.0/Engine29.9.2/client29.2.1, exception rootful explicite pour ces essais. Les paramètres, versions, hashes et limites Linux sont détaillés dans son rapport. Le snapshot Linux précède le seul ajout700 à la fixture ignorée terminal ; code applicatif et quatre suites Linux sont identiques, provenance relue indépendamment. Cela ne qualifie pas un broker sur hôte Linux natif.

## Maintenance QA exécutée

1. Contrôle préalable :17workers tous terminaux,0quota retenu, pas de page manquante, chaîne valide591, tête `f826e4cae9b9b2b18b8e878daf97f2bb84a0bd298f7d7976165e951c4319af5d` ; [preuve](qa-before-maintenance.json).
2. Arrêt ciblé ingress/dashboard/gateway. Inventaire des consommateurs du volume : seul le gateway, arrêté. Aucun broker actif sur cette base. [Arrêt](qa-stop.log), [consommateurs](volume-consumers-before.txt).
3. Copie à froid du répertoire entier, y compris sauvegarde historique, après arrêt. Archive privée hors dépôt `/tmp/ai-guard-sqlite-freeze.DezAml/qa-data-cold-before.tar`, mode600, SHA256 `12cc60d69e84865b1bca96b2ac568a5175f31336e4d836c8514cadef96f3c71a`. Aucun WAL/SHM ne subsistait après arrêt. Sur copie extraite uniquement, SQLite Apple3.54.0 en URI immutable retourne intégrité `ok`, version2,591événements et tête attendue ; [résultat](cold-copy-check-immutable.txt). Une première commande CLI readonly sans immutable a échoué `SQLITE_CANTOPEN(14)` ; elle ne valide rien et n'a pas modifié le volume. Cette particularité du contrôle CLI reste distincte du Store réellement redémarré.
4. Sous UID propriétaire1000, sans réseau/caps et racine readonly, unique commande explicite `chmod 0700 /data`. Aucun chmod récursif, changement de clé, de propriétaire ou de donnée. [Modes avant](volume-metadata-before.txt)/[après](volume-metadata-after.txt), hashes [avant](data-before-permissions.sha256)/[après](data-after-permissions.sha256) identiques, y compris la sauvegarde v1 historique.
5. Nouveau contrôle démarré, trois services sains. Store vérifie sa chaîne591 inchangée ; [après](qa-after-maintenance.json). HTML et favicon OVPT servis identiques au build local. Parent700, main/sidecars/verrou600 observés [sur le service vivant](live-volume-metadata.txt). Aucun schéma migré, claim réarmé ou quota libéré par supposition.

Cette copie à froid vérifiée n'est pas une qualification générale de restauration des claims. Les essais de panne/restauration et fencing restent obligatoires ; aucune ancienne session n'a été relancée depuis la copie.

## Artefact et limites

Déploiement QA : `agentsandbox-control:qa-20261030-sqlite-327-ovpt-8d29af`, ID `sha256:32cdcf1f3719015701966ab6b80da2104281ff1c42226cced79e0f541b9f034b`. Compose `/tmp/ai-guard-registry-qa.40IImn/compose-sqlite-327.yaml`, projet `ai-guard-qa-7dce8f12`. UI OVPT finale conservée. Archive source `/tmp/ai-guard-sqlite-freeze.DezAml/source.tar`, SHA256 `d41832ef03a1651d2d41b8bc160b7f2880de59d469ff5f63a15bc50d1b9bfe10` ; binaire hôte `e60496d63eec4d005e33b0acfddedc97bb70e6ef04a9086d6558449d0433b307`. [Manifeste courant](source.sha256).

Les essais de mise en place restent conservés : fixtures0755 refusées avant adaptation, ajouts `mod support` placés initialement avant documentation interne puis corrigés, helper inutilisé refusé par Clippy puis retiré, erreurs de préparation des sondes Linux détaillées dans son rapport. Ce sont des FAIL de ces tentatives, pas des PASS rétroactifs.

`NOT_RUN` : mounts macOS noowners réels, substitution déterministe A→B→A, toutes ACL/FS/UID hostiles, crash/power-loss/disque plein, restauration sûre de claims, matrice Docker/Podman×Linux/macOS, recetteA1/A2/B1 et Kubernetes/cloud. Les mêmes UID de contrôle, root et administrateurs du stockage/moteur restent hors garantie DAC. La politique réduit la course au pathname sous des parents privés ; elle ne transforme pas le VFS en handoff atomique de FD contre ces acteurs. G06 reste ouvert, comme les autres gates non satisfaits.
