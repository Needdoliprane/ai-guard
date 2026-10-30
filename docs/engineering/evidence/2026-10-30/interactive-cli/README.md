# CLI interactif — transport générique vérifié, clients en qualification

Le [design initial](design.md) reste le document de conception. Le transport PTY Rust est implémenté dans le broker existant ; les preuves finales bornées et leurs limites figurent plus bas. Le candidat QA reste le [lot batchv2 gelé](../engine-logs-v2/README.md), sans ce transport. Les premières sections conservent les étapes historiques et leurs échecs.

**FAIL d'hypothèse, preuve réelle** : le [probe Docker sans start](docker-flags.md) donne Tty/OpenStdin true et StdinOnce true, même avec attaches stdout/stderr explicites. Le conteneur diagnostic est conservé à l'état created. Aucun credential, volume ou fournisseur utilisé. Le [ruling dans DECISIONS](../../../DECISIONS.md) retient le profil Docker réel et l'arrêt sur perte de transport ; il ne prétend pas livrer le réattachement.

Contrat d'implémentation : mode typé optionnel lié à la demande/approbation, preset interactif versionné et historique batch exact ; capture/transcript désactivés, frontière d'affichage terminal et masquage streaming, restauration, resize, lifecycle/audit/arrêt réels. Les tests des contrats, du filtre et du PTY seront exécutés par Luna, puis une qualification Docker générique sans modèle avant tout essai fournisseur borné. Revue indépendante de lancement/audit/secrets requise.

## Premières preuves logicielles, avant gel intégré

Le raccord audit a une reproduction isolée : [RED](audit-red.md),1PASS/2FAIL attendus (mode absent et mode masquable accepté), puis [GREEN](audit-green.md),3PASS après ajout de la persistance et des gardes. Les anciens champs batch restent omis ; le hash d'approbation est distinct pour le canal interactif. Ces copies ne qualifient pas le runtime TTY.

Sur le checkout d'intégration, [Luna](initial-tests.md) a ensuite obtenu5PASS du filtre/PTY local et3PASS d'audit. Hashes des fichiers concernés identiques avant/après exécution ; [log terminal](initial-terminal-lib.log), [log audit](initial-decision-audit.log). Le code continue d'évoluer ensuite, notamment restauration/annulation et classification des erreurs ; aucun PASS final attribué par anticipation.

## Revue, corrections et qualification générique

La [revue indépendante](review-before-pty-fix.md) a trouvé deux P2 : abandon de la queue PTY à la fin du client moteur et marges de défilement non restaurées. Le [RED des marges](scroll-reset-red.log) a échoué avant correction ; le reset corrigé passe ensuite. Les tests intermédiaires de contrats/audit/registre/reprise passent dans leur [périmètre détaillé](software-intermediate.md).

Le premier correctif de drain **ne passe pas** les deux nouvelles sondes PTY macOS : [9PASS/2FAIL](first-drain-fail.log), queue vide et erreur de filtre perdue. La lecture statique favorable à ce delta ne remplace pas ces FAIL. Les sources XNU indiquent flush au dernier close du slave et révocation du terminal contrôlant à la sortie du leader ; cause/correctif à confirmer par tests. Implémentation en reprise, aucun passage moteur lancé pour contourner cet échec.

Le [harnais Docker](docker-test-preparation.md), ses helpers Rust et l'image probe dédiée sont préparés. Root a fait corriger quatre défauts de test avant exécution : vérifier termios avant nettoyage, inspecter le terminal avant ACK/cleanup, exiger une sentinelle après les séquences adversariales et observer réellement le resize. Un test qui n'a jamais reçu le suffixe ne prouve pas le filtrage de ce suffixe. Le build est autorisé localement ; les runs attendent la correction et la revue du transport.

Ces constats précèdent les corrections suivantes, qui ne les effacent pas :

- Le transport garde sa session sans controllingTTY, garde explicitement un slave jusqu'au drain EAGAIN puis exige EOF/EIO réel. Les erreurs finales restent nommées, les suffixes privés masqués et les marges réinitialisées. Le même scénario de queue après réap passe ensuite.
- Le premier vrai Docker a révélé l'alias `/dev/tty` Darwin non enregistrable en kqueue : [sonde](tty-fd-probe.md), [RED sur vrai code](tty-device-red.log), puis ouverture indépendante d'un device réel avec identité/session/foreground vérifiés avant admission. Il s'agissait d'un défaut produit, accompagné d'un défaut de diagnostic du harnais ; ce premier run ne devient pas PASS.
- Le harnais a ensuite bloqué après une fin produit correcte. Il avance désormais dans une boucle poll unique, sans thread/join, avec vérification avant ACK au driver. Le smoke sans moteur a terminé en0,34s. La cause exacte de l'ancienne attente n'est pas affirmée au-delà du sample et des traces disponibles.

**PASS logiciel sur dernier gel** : terminal12/12, autorisation8/8 et cibles contracts/registry/recovery nommées dans [software-final.md](software-final.md), fmt/Clippy alltargets/release. Les deux FAIL initiaux termios ont leur cause et correction de fixture documentées, sans masquer les flags. Pas de nouvelle suite globale présentée comme exécutée. [Revue du dernier device](review-device.md), aucun bloqueur restant pour qualifier le CLI sur Docker ; précondition Tokio I/O et réserve d'ACK lent conservées.

**PASS Docker Desktop/macOS ARM64**, deux sessions fraîches :

| Cas | Résultat observé |
|---|---|
| Clavier, resize, exit7 | 1PASS/12,89s. Trois fd TTY, taille80×24→120×40 vue par le worker, saisie acquittée, contrôle des séquences avant sortie, exit7 réel conservé comme échec CLI, restauration termios avant ACK driver. Session `5d28f56a-352a-484f-a0af-4a30d780e9b7`, worker `9dbe3447-96af-4dec-8de2-cda01cfaa858`, CID `b7cf436ce49ea8a9b9d1b31c6a1e3e13f1c183d12836d68203db25597fbe177f`. |
| DoubleCtrlX | 1PASS/13,90s. Arrêt du worker détenu, terminal Docker observé avant ACK, une autorisation d'attachement/start, restauration avant ACK driver. Session `f13b97b2-eab6-4ac7-9cba-4ca60bcdffbc`, worker `7638fc7c-5a36-4246-b178-c38ead1745ce`, CID `244730457fed800a3ab5fd5fad2215acaa3c386aa3cef60d8692fb0915245d24`. Aucun témoin concurrent : pas de preuve d'isolation face à un autre worker actif. |

Dans chaque cas : audit applicatif valide après réouverture (25événements), une allocation observée `billable=false`, quota libéré après ACK, cleanup normal et original inchangé. [Rapport complet](docker-real.md). Les deux runs antérieurs restent FAIL. Les sorties Cargo originales de ces PASS demeurent dans l'historique outil ; le fichier nommé `ctrlx-cargo.log` est un **résumé scalaire reconstitué**, pas leur stdout brut. Aucun transcript PTY n'est archivé. Les futurs runs doivent conserver directement leurs sorties Cargo scalaires privées.

**Réserve de revue finale sur l'ancien CtrlX :** ce premier test n'assertait ni le motif durable `transport_stop_reason` opérateur, ni `timed_out=false`, ni un délai distinct du timeout worker120s. Le temps réel13,90s est cohérent avec l'arrêt clavier, mais ses seules assertions pouvaient aussi accepter un arrêt au timeout. L'ancien résultat reste daté avec cette limite ; aucun défaut de fonctionnement CtrlX n'était établi par cette lacune du test.

**Renforcement et unique rejeu PASS1/1 en13,57s :** le test exige maintenant le motif exact `operator requested worker stop with Ctrl-X Ctrl-X` dans l'audit rouvert, `timed_out=false`, une réponse broker en moins de30s après la seconde frappe, un worker terminal, quota0 et une allocation unique observée non facturable. [Sortie Cargo originale](logs/ai-guard-interactive-ctrlx-strict-cargo.log). Le délai précis après frappe n'a pas été imprimé ; seule l'assertion et la durée totale sont conservées. Session `6bed5970-514c-4863-8c23-d90169b6dec7`, worker `d3ea8183-b60a-4495-bbf1-13d3402cf79e`. Test SHA256 `3c8d7dfd48e1738f177493ccd2d49021ff2b3fed68f0f81775a34b4aee15810b`, production/helper inchangés, ancienne image probe `ea216cf7…` vérifiée. La [revue finale](review-final.md) ferme l'écart d'assertions, sans témoin concurrent ni nouvelle preuve d'isolation inter-workers.

Source privée gelée : `/tmp/ai-guard-interactive-qualified.hO9L2x/source.tar`, SHA256 `348a6c2aeeb7a015114e2c47f8794e41a67b512ccbac27b86d4402d6dfe33207`. Binaire macOS SHA256 `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`. Terminal `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, test réel `188377b622ebb70e0e6e0707916b99c015ed795ff9f98ce61a96ac1c1c2e1a61`. Image probe `sha256:ea216cf78c8e34275fc9e3a7cf309231c151b9653253a86f65875a261ec15588`, dérivée de la base vérifiée du rapport. Rust1.99.1, Docker client29.2.1/moteur29.9.2, Darwin27.2 ARM64, exception rootful explicite. Le SDK était présent : un échec de build venait d'un PATH de harness incomplet, corrigé et relancé PASS, pas d'un blocage externe.

### Troisième cas : octets clavier distants exacts

**PASS1/1 en12,29s**, sans fournisseur ni réseau : dans le vrai worker, `stty raw -echo` puis lecture des huit octets publics `0d 0a 03 c3 a9 1b 5b 41` (CR/LF/Ctrl-C/é/flèche haut), comparaison hexadécimale exacte, sortie0. Restauration hôte vérifiée avant ACK, terminal Docker inspecté, allocation observée unique non facturable, quota0, audit25 valide après réouverture et original intact. [Log Cargo original fusionné](logs/ai-guard-interactive-rawkeys-cargo.log), sans transcript PTY. Session `7ccca22c-3d66-4dc5-8469-b82cacf48177`, worker `c9715f2e-3a62-4e11-9e77-49ea64e95396`.

Production inchangée depuis le gel ci-dessus. Test mis à jour SHA256 `78e18be1a808265dc95579b9d65549731d7d44e7aa742cc7a4222dbd6381d0f6`, helper `9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`, nouvelle image `agentsandbox-worker:interactive-raw-keys-qa-20261030`, ID `sha256:c16828672d0e01049b91b8bbe3d993e213eff7557076a9552908dd24d1e795bb`. L'ancienne image et ses deux résultats ne sont pas réattribués au nouveau helper. Commande et preuves complémentaires dans le [rapport Docker](docker-real.md). Ce cas ne démontre pas que Ctrl-C interrompt une application distante en mode canonique : le helper est volontairement raw pour comparer les octets.

### Reproduire les deux cas sans fournisseur

Le Dockerfile de fixture lit son helper depuis le **contexte tests/runtime_fixtures**, pas depuis la racine exclue par `.dockerignore`. Vérifier d'abord l'ID de base attendu dans le rapport, puis construire une nouvelle image de test sans retagger une image existante :

```sh
docker build --pull=false --network=none \
  --file tests/runtime_fixtures/interactive.Dockerfile \
  --tag agentsandbox-worker:interactive-qualification \
  tests/runtime_fixtures

AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock \
AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qualification \
CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive \
  docker_interactive_cli_forwards_tty_input_resize_and_worker_exit -- --ignored --exact

AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock \
AGENTSANDBOX_TEST_INTERACTIVE_IMAGE=agentsandbox-worker:interactive-qualification \
CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive \
  docker_interactive_double_ctrl_x_stops_only_the_owned_worker -- --ignored --exact
```

Le harnais est actuellement épinglé à ce Docker Desktop/macOS local et au programme `/opt/homebrew/bin/docker`. Cette recette ne prétend pas qualifier Linux ou Podman. Les deux cas sont ignorés par défaut ; l'entrée interne `interactive_host_driver` est une fixture, pas un test produit indépendant à compter comme PASS. Les dossiers et volumes de diagnostic sont conservés. Le build/les tests ne nécessitent aucune clé fournisseur.

**NOT_RUN dans ce dossier** : vrais Codex/Claude TUI (chantier suivant), Podman/autres plateformes, Kubernetes, charge100, recette A1/A2/B1, panne active et restauration. **Écarts ouverts** : réattachement après claim consommé, authentification Claude autorisée dans worker, durée autonome après perte du broker. Aucun résultat n'est remplacé par le PASS du Codex batch ; aucun gate produit clôturé par ce seul chantier.
