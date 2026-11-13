# Raccord du relais au broker — contrat de développement

[English](BROKER_HANDOFF.md) · [Client du relais](README.fr.md) · [Provisionnement workload](../workloads/README.fr.md)

Ce document décrit les sources en développement du raccord au broker.
Le 13 novembre 2026, le candidat `223b699e959964bef4460f34857516cb8cdd37c8` passe
build lib-test, check toutes cibles et cinq parents normaux de settlement de
retraite broker en QA isolée ([preuves](../../engineering/evidence/2026-11-13/broker-full3/README.fr.md)).
Ces parents n'exercent pas le transport relais décrit ci-dessous et ne qualifient
pas son chemin runtime. Les preuves antérieures du propriétaire de contrôle, du
port runtime et du SDK Core restent attachées à leurs propres sources. Aucun
moteur réel, modèle, service cloud ou essai de100 workers n'a été lancé pour ce lot.

Le raccord activé explicitement relie le vrai callback Running du broker et la
reprise du WorkerClaim durable à l'admission Core, à l'exec fixe sans TTY du
helper et au propriétaire de contrôle retenu. Sans configuration de relais,
celui-ci est indisponible. Une déclaration de profil ne prouve jamais Ready.

## Références fixes et émission après Running

Seul un profil du registre broker de version 2 accepte ce fragment facultatif :

```json
"relay": {
  "version": 1,
  "workload_operations": ["/absolute/private/workload-operations/worker-operation"]
}
```

Remplacer ce chemin illustratif. Le profil existant, l'authentification runner,
le choix du runtime et ses restrictions restent appliqués. Les références sont
des chemins absolus uniques et bornés, de 1 à 100 par profil. Leurs parents
privés doivent déjà exister, être retenus sans symlink et être disjoints de
toutes les racines configurées de source et d'export. L'opération enfant peut
être absente au départ. Le broker ne la crée, répare, émet ou renouvelle jamais.

L'opérateur fixe la référence de l'opération dans le profil avant l'admission.
Après le vrai passage du Worker à Running, `broker relay-status` identifie son
worker, sa tentative et sa génération exacts à partir des faits authentifiés
retenus par le broker. Utiliser cet endpoint dans le manifeste workload fermé
existant, puis les commandes existantes
`workload prepare --input … --control-file … --output-dir …` et
`workload apply --operation-dir … --control-file …`. Le guide workload précise
le manifeste et l'identité de contrôle autorisée. Ne pas déduire les identifiants
d'une frame du worker ni substituer un credential humain ou fournisseur. Les
grants dirigés doivent eux aussi avoir été autorisés explicitement par le contrôle.

Le broker attend uniquement sur les références configurées. Un enfant absent,
une préparation incomplète vérifiée ou un verrou de préparation actuellement
détenu permettent un nouvel essai. Un contenu malformé, des permissions non
sûres, un FD/inode changé, une divergence de périmètre, digest, reçu ou binding
produisent un refus explicite. Un ensemble complet sans endpoint correspondant,
ou deux opérations correspondantes, est refusé. L'opération choisie reste en
lecture seule, avec ses FD retenus. Aucun cache OAuth ni provisionnement
implicite n'est utilisé. Le provisionnement automatique reste un écart produit.

L'attente et tous les effets suivants emploient la méthode d'horloge continue,
le boot ID et l'échéance absolue de l'Arm Worker originale acquittée. Ni les
métadonnées UTC ni la cadence des polls ne renouvellent ce délai. STOP et
l'expiration interrompent l'attente ; aucun mutex IO du tenant n'est détenu
pendant le délai entre polls. Un reçu apparu trop tard ne permet pas d'exec.

## Rétention, diagnostic et échecs

La nouvelle commande source reste distincte des Status/Page V1 historiques :

```text
agentsandbox broker relay-status --root /absolute/private/broker-root \
  --profile configured-profile --token-file /absolute/private/runner-file \
  --operation-id EXISTING-JOB-UUID
```

Ce sont des valeurs à remplacer et une interface source, pas une recette
exécutée. Le service authentifie le même profil, tenant et namespace que le
status existant et résout son job accepté dans le journal avant les détails du
relais. La réponse fermée et versionnée expose AwaitingIssuedWorkload, Admitted,
Unknown ou Closed, l'endpoint et le canal retenus, leur provenance, l'heure de
dernière observation et des erreurs fermées. `freshness` décrit explicitement
des faits retenus, jamais une sonde runtime fraîche. Aucun credential ID, preuve
ou chemin privé d'opération n'est renvoyé. Admitted ne prouve pas Ready ; l'état
Core est distinct. Une erreur n'est ni une inbox vide ni une absence d'activité.

Le Status V1 historique conserve ses champs fermés. Status et RelayStatus
peuvent utiliser la dernière projection confirmée par le même acteur writer,
initialisée par une lecture stricte réussie du journal. Elle n'est publiée
qu'avec un acquittement réussi de cet acteur, après la relecture durable du
journal ; une mise à jour en cours conserve sa tête précédente confirmée.
Ce diagnostic retenu n'est ni un nouveau contrôle d'intégrité du filesystem,
ni une sonde moteur à chaque poll. Un job froid ou non retenu emprunte le
chemin strict ; une absence dans le cache ne signifie jamais un job absent.

La projection mémoire conserve au plus 256 jobs acceptés et 4 Mio de données
de projection comptées par writer, avec les bornes jobs/périmètre du journal
si elles sont plus strictes. Elle ne contient ni binding fournisseur,
historique, credential ou permission d'exécution. Une projection trop grande
emprunte le chemin strict sans tronquer ses champs. Ces bornes logiques ne
qualifient ni le RSS mesuré ni 100 workers. Échec d'intégrité d'une lecture
stricte, publication inconnue, fencing/arrêt du service ou perte de son owner
acteur/service ferment ce port de diagnostic. Une erreur ne signifie pas
zéro activité. Préserver les enregistrements et employer les diagnostics de
reprise ; aucun fait du cache n'autorise claim, start, mutation, export ou
libération du quota.
Une couverture STOP partielle peut coexister avec une opération en attente
confirmée, par exemple une demande Arm sans ACK local. Elle ne constitue ni
une erreur d'intégrité ni une preuve de santé runtime ; les constats d'intégrité
typés ferment toujours le diagnostic retenu.

Seul un ClaimExec Core frais reçu en HTTP 201 peut être consommé pour un exec
du helper. Un rejeu historique ne recrée jamais cette capacité. Le binding
courant et l'incarnation/configuration runtime exacte épinglée sont revérifiés
avant exec et dispatch. Le helper ne reçoit que des frames fermées via l'exec
séparé, fixe et sans TTY du runtime existant. Aucun secret workload/contrôle ou
identité locale n'est placé dans les frames. Le worker garde son réseau isolé.

Un tenant possède un outbox verrouillé et un propriétaire IO sérialisé, partagés
entre ses profils configurés ; les clients et autorités restent propres à chaque
canal. Les identités immuables de send/ACK et leurs marqueurs de dispatch précèdent
HTTP. Les reçus confirmés précèdent le succès local. Une reprise garde les mêmes
identité et contenu ; effets inconnus, révocation et EOF n'effacent jamais la
demande. Le nouveau namespace `relay_channels_v1` retient des faits de métadonnées
bornés à côté de `relay_v1`. Ce journal privé et versionné n'est ni une nouvelle
base ni un checkpoint d'audit/signature. Des fichiers corrompus, partiels, d'un
ancien périmètre ou modifiés ferment l'accès : les préserver, jamais les supprimer
ou les réparer seulement pour rouvrir.

Les IO bloquantes sont admises avant `spawn_blocking` par des permits tenant et
globaux réellement partagés, de 4 et 16 segments. Le travail en attente ou en
cours conserve permits, FD/verrou, RoleLease et réservation logique d'octets
jusqu'à sa fin réelle. L'annulation retient le JoinHandle et interdit son
remplacement avant join explicite. Les résultats inconnus porteurs de ressources
restent au registre. Joins locaux, EOF du helper et sortie CLI ne prouvent pas
un Worker terminal et ne libèrent pas son quota. La fermeture Core exige la
révision et l'identifiant d'observation terminale durables exacts du Worker.

Une intention de retrait retenue est publiée Unknown avec
`reconciliation_required=true` tant qu'un acquittement Core compatible n'est
pas aussi durable dans le journal local. La dernière erreur connue reste
visible pendant cette incertitude. Refus, réponse perdue, contrôle indisponible
ou échec de sauvegarde locale ne publient ni Closed ni une attente normale du
workload. Même sans admission Core, la fermeture locale n'est publiée qu'après
son commit dans le journal. Après une sauvegarde interrompue, le retrait joint
l'owner IO existant et relit le fait local durable avant de préparer une autre
intention ; il ne recrée aucun claim d'exécution et ne renouvelle pas
l'échéance d'origine.

Les dénominations de 3 MiB par échange et 32/128 MiB tenant/global sont des unités
logiques d'admission. Inbox décodée, valeurs JSON et frames encodées peuvent
coexister au-delà de cette dénomination. La mémoire totale/RSS et la capacité FD
sont NOT_RUN. Les limites de frames et de stockage restent indépendantes.

## Vérification et reprise encore ouvertes

Les tests source couvrent l'horloge/boot/STOP originaux, la réouverture des faits
durables et le refus des fichiers partiels, le préfixe UDS inactif sans réservation
de corps et le canari du Status V1 historique. Ils sont NOT_RUN pour cette
composition. Les deux parents logiciels avec Core/HTTP/SQLite, deux endpoints
partageant l'IO tenant, le vrai helper Rust et un runtime témoin explicitement
synthétique restent à écrire et exécuter en QA isolée. Le témoin ne pontera que
le stdio de l'exec par une UDS privée de fixture ; il ne qualifiera aucun moteur
réel et ne changera pas les arguments du helper de production.

La source de retrait utilise une lecture historique distincte après le Worker
terminal exact et la jonction locale admissible. Le runner authentifié et
l'admission retenue fixent tenant, namespace, endpoint et identités d'opération.
Cette lecture n'appelle jamais Reserve ou ClaimExec et ne crée aucune autorité
d'exécution fraîche. Les anciennes intentions Claim, Ready et autres intentions
d'attachement restent immuables, y compris celles remplacées dans les anciens
faits V1. Elles sont résolues par reçus exacts ou preuves d'inapplicabilité
validées ; le seul snapshot du canal ne constitue pas leur reçu.

Le cleanup est une intention durable distincte : CancelReserved pour un canal
Reserved confirmé, ou CloseExactWorkerTerminal pour un canal admis non terminal,
avec CAS du canal confirmé et preuve terminale Worker exacte. Une réponse de
cleanup perdue conserve le même UUID/contenu. Son reçu historique est recherché
d'abord ; seul ce cleanup encore applicable peut être rejoué explicitement.
Les anciennes intentions d'attachement ne sont jamais rejouées pour découvrir
leur résultat. Tous les pending doivent être résolus avant Finalize Closed du
préfixe local complet. Une preuve exacte d'admission jamais enregistrée permet
de fermer sans inventer un canal Core. Un historique purement local sans
admission, intention ou état Core exige aussi preuve terminale et join avant
finalisation.

Le même journal privé accepte des records V1/V2 append-only. Les octets V1
existants ne sont pas réécrits ; un Closed V1 historique ne devient ni nouvelle
preuve V2 ni capacité exec. V2 réserve 48 records par job : au plus 16 records
ordinaires/hérités et 32 réservés aux progrès de retrait validés. Le plafond du
tenant est de 12288 records et chaque enveloppe canonique complète reste bornée
à 16 KiB. Polls ou heads inchangés ne consomment aucun record. Le cleanup réserve
sa preuve et sa finalisation avant HTTP ; une capacité épuisée conserve Unknown,
les records et leurs owners. Ces bornes logiques ne qualifient ni RSS,
performance du stockage physique ou rétention illimitée. Un ancien binaire
refuse un préfixe contenant V2 : conserver le préfixe/sauvegarde d'origine avant
mise à jour ; ne pas supprimer de records pour rendre un downgrade possible.
Cette migration ne fournit pas d'ancre anti-rollback externe.

Un fait repris avec `local_joined=false` ne prouve pas le join d'un ancien CLI
par l'absence de handle. Preuves absentes, incohérentes ou CAS futurs non pris en
charge restent Unknown, y compris un canal terminal dont un ancien pending ne
peut être résolu. Un Worker terminal ou un redémarrage du broker ne clôturent
pas seuls ces cas. Reprise runtime, pannes physiques et wrapper complet de
job/service vérifié gardent leurs preuves propres ; les tests logiciels du
consumer historique ne les qualifient pas.

Revue indépendante, compilation/tests isolés sur sources gelées, parent IPC de
refus du périmètre avant détail, recettes moteur réelles, qualification du profil
HTTPS/AC privée et mesure à 100 workers restent ouverts. Ce lot ne qualifie ni
le parcours opérateur complet ni une release de production.
