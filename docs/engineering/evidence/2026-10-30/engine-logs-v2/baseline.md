# Docker worker log baseline — lecture ciblée

Date : 30 octobre 2026. Inspection read-only du conteneur de qualification fourni; aucun conteneur/image n’a été créé, démarré, modifié ou supprimé. Aucun appel fournisseur effectué.

## Instance vérifiée

Conteneur b37ea5ddd5dfecf0a49cdbe2a0583376152b64f780315c1fc3182ef2566f3ba7:
- État exited, exit 0, OOMKilled false; créé 2026-10-30T00:48:07.31709342Z, terminé 2026-10-30T00:48:08.166917795Z.
- Image ID sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6; image/tag de qualification connu agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984, raccordé à l’audit via la preuve locale du worker de test.
- Labels ciblés : ai-guard.qualification=runtime-resume-20261029; agentsandbox.purpose=run; session bade333d-38f2-4d7c-969c-2a553f4a81c4; opération e5856a5e-58e5-44d3-b4b4-7668041e6789.
- Docker Engine 29.9.2; daemon DefaultLoggingDriver=local.
- Configuration effective relevée sur le conteneur : LogConfig.Type=local, LogConfig.Config={}. Donc aucune option de rotation explicitement attachée au conteneur.
- docker logs <ID> 2>&1 | wc -c a donné 5 octets. Le contenu n’a pas été affiché, enregistré ni inspecté.

Le lien image de qualification est corroboré dans docs/engineering/evidence/2026-10-29/local-final-8bd72c0e/runtime-confinement-report.md, qui documente l’image/tag et son usage de test. Le statut/launch correspond à une session strict, réseau none, commande /usr/bin/id dans les preuves locales du worker. Les chemins de preuve privés ne sont pas reproduits ici.

## Interprétation bornée

Le driver effectif est local, pas json-file et donc pas le cas sans rotation par défaut. Les options vides signifient que le conteneur ne surcharge pas les valeurs natives du driver. La documentation officielle Docker décrit pour local une taille par fichier par défaut de 20 MB, 5 fichiers au plus, rotation automatique et compression; plafond nominal de conservation de 100 MB par conteneur avant compression. Source : https://docs.docker.com/engine/logging/drivers/local/. Le docker info local confirme séparément local comme driver par défaut du daemon. Ce constat est celui de cette instance/Engine; il ne prouve pas que tous les workers ou moteurs configurés différemment ont ces valeurs.

Cela fournit une baseline bornée pour le nouveau preset : le précédent conteneur de qualification avait accès au driver local, lequel conserve des logs lisibles via Docker et a des bornes par défaut. L’inspection ne prouve pas l’absence de sortie log sensible au-delà des 5 octets observables ici. Aucun log brut n’a été lu.

## Méthode et confidentialité

Seuls les champs Docker sélectionnés (ID, image, état, LogConfig et labels qualification/runner/session/opération) ont été rendus. Aucun champ Config.Env, secret, docker inspect brut, volume ou montage n’a été demandé. Le flux docker logs est passé directement à wc -c; seul le compte d’octets est affiché.

Fichier privé de constat : /tmp/ai-guard-engine-logs-baseline.md. Les détails ci-dessus ne nécessitent pas l’archivage du contenu logs.
