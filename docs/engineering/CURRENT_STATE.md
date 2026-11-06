# État constaté — suivi du développement

## 6 novembre — cohérence des guides local, Keycloak et SSO

Les sept guides du lot `4195450`, composé dans `bca854f`, distinguent l'accès
technique runner/viewer, le provisionnement des comptes nominatifs et leurs
lectures/exports autorisés. Ils renvoient au mapping OIDC exact `(issuer, sub)`
et à la maintenance des schémas4–10. Le lancement humain via le broker reste
proposé ; une connexion ne l'autorise ni ne lui attribue une exécution technique.

Revue indépendante et contrôles documentaires ciblés passent ; aucune procédure
produit, connexion IdP ou exécution runtime n'est relancée pour ce lot de texte.
[Validation](VALIDATION.md#6-novembre--cohérence-des-guides-local-keycloak-et-sso).
Aucune fonctionnalité supplémentaire ni gate global qualifié.

## 6 novembre — reprise de présence après panne temporaire du gateway

Le lot `9f3f96d`, composé dans `f0fa2ea`, garde le même publisher, client,
identité épinglée et Pending après les quatre premiers refus temporaires.
Après les trois attentes courtes, la cadence devient `min(interval_seconds,30)`
secondes, interruptible ; seuls Transport/Unavailable reprennent. Refus de
droits, conflit, horloge, protocole, capacité et matériau privé restent terminaux.

Dix-sept fonctions ciblées passent ; l'observation manuelle de coût ignorée
reste NOT_RUN. Le check toutes cibles du lot puis de sa composition TLS passe,
revue indépendante traitée et guides EN/FR synchronisés.
[Preuves](evidence/2026-11-06/presence-recovery/README.md).
Ce résultat n'établit ni fédération physique multi-hôtes, inventaire runtime,
arrêt de worker, reprise/fencing complet ou capacité100. G09 FAIL,0 gate clos.

## 6 novembre — confiance HTTPS explicite du CLI humain

Le lot `68f0fb4`, composé dans `f295908`, ajoute `--gateway-ca-file` au CLI
local et à `human sso`. Le bundle public borné est chargé avant credentials
ou login ; le même snapshot suit admission, lectures/export, refresh et logout
ai-guard. La confiance IdP reste distincte. L'export lie les octets du bundle
à sa reprise ; les digests du mode par défaut restent compatibles.

Dix fonctions ciblées passent après le RED d'option absente, ainsi que le
check toutes cibles ; revue indépendante traitée et guides EN/FR synchronisés.
[Preuves et limites](evidence/2026-11-06/human-gateway-trust/README.md).
Les avertissements118 lib,3 lib-test et2 de fixture partagée restent visibles ;
Clippy est BLOCKED dans l'image QA. Les tests TLS locaux ne qualifient ni PKI
d'entreprise, Keycloak, navigateur, clients IA ou transport multi-hôtes réel.
G09 FAIL ; aucun gate global clos.

## 6 novembre — confirmation des ACK en quarantaine

Le lot `7b630ff` permet au même acteur Stops de confirmer son lot exact déjà
scellé, sans republier un checkpoint, effectuer de HTTP, créer un droit Full
ou libérer sa lease. Une preuve incomplète conserve l'input et sa réservation.
Absence ou répétition renvoie None, sans preuve d'ACK. Le service tente cette
confirmation sur Busy avant tout Job ou nouvel envoi ; les autres profils
continuent après la fin de la pagination de l'acteur concerné.

La régression de progression trouvée en revue a son RED strict avec contrôles
valides, puis son GREEN : B commet et confirme ses deux événements malgré
l'erreur A. Seize fonctions ciblées passent sur leurs gels, revue traitée et
check toutes cibles RC0,118 avertissements lib/3 lib-test.
[Preuves](VALIDATION.md#6-novembre--confirmation-de-quarantaine-sans-republication).
Tentatives non scellées, cible non prouvée, perte du processus, reprise froide
complète et D5/fencing restent ouverts. Aucun moteur réel ou gate global
n'est qualifié ; G09 FAIL.

## 6 novembre — registre workers pour les identités humaines

Le lot `1bede3b`, composé dans `b14b9ce`, ajoute une lecture administrative
du registre workers avec permission explicite `read_worker_registry`, identité
locale nominative ou OIDC admise, contexte et tenant vérifiés côté Rust. La
projection bornée et son reçu durable partagent les budgets humains existants.
Révocation, curseurs distincts, refus avant publication et purge de la vue sont
traités. La migration9→10 est explicite, avec sauvegarde ; elle n'accorde aucun
droit et n'active pas l'exécution. [Guides EN](../setup/identity/WORKERS.md)/[FR](../setup/identity/WORKERS.fr.md).

Les régressions couvrent aussi UUID historiques, dates inconnues ou hors format
et code de sortie i64 exact sous forme de chaîne. Quarante-cinq fonctions Rust
et quarante-six Node uniques passent sur leurs gels ; tsc et compilation de
la composition RC0, revue indépendante traitée. [Preuves](VALIDATION.md#6-novembre--registre-workers-humain).
Build frontend complet et navigateur restent BLOCKED par l'outillage offline.
Cette vue administrative ne clôture pas la vue personnelle : l'attribution
authentifiée initiateur/délégation/admission reste obligatoire. Transport
multi-hôtes, relais, capacité100 et moteurs réels restent à qualifier.
G09 FAIL, aucun gate global clos.

## 6 novembre — propriétaires Full et admission avant accusé de réception

Le lot `5026f07` retient les futures Full acceptées jusqu'à leur fin et attend
leur drainage avant de libérer la lease du broker à l'arrêt normal. Chaque
admission réserve son propriétaire avant mutation ou ACK ; une saturation
renvoie `Busy` sans nouvelle commande Resume ni ticket Submit. Les replays
exacts acceptés ou refusés restent lisibles à capacité pleine, et un contenu
divergent reste un conflit. Le registre Full et le registre Cold partagent un
mécanisme privé typé, sans conversion de leurs droits.

Quatre parents Full et cinq canaris Cold passent sur leurs gels. La course
d'admission découverte en revue a son RED et son GREEN ; l'oracle final exige
explicitement l'absence d'événement Resume. Compilation toutes cibles RC0 et
contre-revue traitée ; [preuves et limites](VALIDATION.md#6-novembre--propriétaires-full-et-admission-avant-ack).
La composition avec les autres lots est vérifiée séparément. Une tâche finie
ne prouve pas un runtime arrêté : SIGTERM réel, mort du processus, reprise
froide, fencing et D5 restent ouverts. G09 FAIL, aucun gate global clos.

## 6 novembre — projection centrale workers et brokers

Le gateway expose une lecture versionnée séparée des WorkerV1 historiques.
Une transaction par page associe les workers du tenant authentifié aux
propriétaires historiques de leurs autorités de lancement et d'inspection.
Le dashboard technique affiche provenance, incarnation courante, fraîcheur,
divergence runtime/grant et limite de migration, sans inventer un inventaire
runtime. Les workers des schémas2–5 restent visibles avec une limite explicite.

Trois parents Rust, le canari avec identité worker réelle, la lecture TS des
trois réponses HTTP réelles,24 tests Node et tsc passent. Revue indépendante
traitée ; compilation toutes cibles après correction de ressource. Les erreurs
de fixtures, TypeScript et OOM sont séparées dans les
[preuves](VALIDATION.md#6-novembre--projection-fédérée-du-registre).
Build frontend complet et navigateur BLOCKED par l'outillage offline absent.
Vue humaine SSO unifiée, transport inter-workers, connexion physique multi-hôtes,
fencing, sessions Kubernetes et qualifications restent obligatoires et ouverts.

La nouvelle exigence P1 réseau est prise en compte : autorisations de
destinations, audit avant connexion, révocation et couverture vérifiable sur
les trois moteurs. Le mode bridge existant n'est pas ce contrôle ; la
proposition de frontière avance séparément. G09 FAIL,0/12 gates clos.

## 6 novembre — prérequis Kubernetes consommés par le CLI

`kubernetes doctor --check-permissions` interroge les douze actions fixes du
profil via SelfSubjectAccessReview. Le rapport distingue namespace actif,
terminating/inconnu, avis allowed/denied/unknown et vérifications sautées.
Le diagnostic sans option conserve ses trois GET et son rapport historique.
Préflight et CREATE retenu refusent désormais un namespace non actif avant
et après l'opération ; les avis SSAR ne créent aucune capacité d'activation.

Le préflight acceptant Terminating a son RED local TLS. Vingt-deux fonctions
ciblées passent, avec compilation toutes cibles et revue source ; les deux
oracles doublon/profondeur ont été rendus discriminants et leur parent repassé.
Les [preuves](VALIDATION.md#6-novembre--prérequis-kubernetes-explicites)
distinguent ce peer déclaré du cluster Docker Desktop, toujours NOT_RUN ici.
Sessions complètes, stockage exclusif, start/release, confinement et reprise
restent ouverts. G09 FAIL ; aucun gate global clos.

## 6 novembre — drainage des propriétaires d'audit à l'arrêt normal

Le service ferme le registre borné des tâches d'audit avec leur admission,
joint le producteur puis tous les propriétaires acceptés. Les handles restent
retenus si un observateur abandonne l'attente ; la première erreur demeure
visible après reap. Aucun timeout global n'abandonne ces tâches comme un succès.
Un sous-ensemble reçu et confirmé reste une fin sûre partielle : le Job conserve
NeedsReconciliation et l'événement non envoyé reste pending. Un ACK incertain
ou une tâche en erreur produit un arrêt dégradé après les autres joins.

Six fonctions ciblées passent : trois unitaires du registre et trois parents
HTTP/SQLite (drain, propriétaire partiel, boucle complète), puis compilation
toutes cibles RC0. La revue indépendante est traitée. [Preuves et limites](VALIDATION.md#6-novembre--drainage-des-propriétaires-daudit).
Les 118 avertissements bibliothèque/3 lib-test et Clippy absent restent ouverts.
Cette preuve interne ne qualifie pas SIGTERM réel, mort du runtime, deadline
produit10s, panne physique, D5/fencing, ni moteur réel. Les exécutions Full
DeferredRun gardent leur contrat indépendant ; G09 FAIL,0/12 gates globaux clos.

## 6 novembre — propriétaire authentifié de l'audit retenu

Le chemin logiciel ServiceCold est raccordé au vrai backend Stops, avant la
barrière d'admission métier qui reste fermée. Le propriétaire détaché conserve
ses réserves avant HTTP, authentifie le même client, vérifie scope et Worker
terminal, puis soumet le sous-ensemble exact de reçus sans nouvelle attente.
Il n'ouvre aucun Full/contexte métier et ne touche ni runtime, provider, Arm/Claim,
Cancel ou finalisation gérée. Les guides broker EN/FR expliquent son usage.

Les deux parents finaux passent : vraie boucle ServiceCold et owner partiel,
avec compilation toutes cibles RC0 et revue indépendante sans blocage restant.
Le vrai RED de publication,
la mutation de perte du premier reçu, les erreurs de harnais/ressources et le
check initial sont conservés séparément dans les [preuves](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte).
118 avertissements bibliothèque/3 lib-test et Clippy absent restent ouverts.
Le drainage ciblé ci-dessus est depuis vérifié. Sortie de quarantaine,
budget10s du writer, D5/fencing et qualifications réelles
restent ouverts. G09 FAIL,0/12 gates globaux clos ; aucun statut de release.

## 6 novembre — ACK retenu sur l'acteur Stops

La façade privée du vrai backend Stops retient la lease et les réservations
avant HTTP, puis enfile la complétion sans nouvelle attente d'admission. Le
merge préserve les Cancel, refuse les collisions de séquences et les divergences
de scope/historique. Un seul lot peut publier sous la même réserve. Après une
erreur, l'acteur retient l'input avant de répondre ; cette quarantaine ne garde
pas son sender et n'empêche pas la fermeture de la queue.

Le gel composé passe 17 parents : dix nouveaux et sept canaris budget. Ils
utilisent de vrais journal/HTTP/SQLite, avec un peer déclaré pour les réponses
incohérentes. Perte d'attente, erreur BeforeAck, persistance et réouverture de
lease sont vérifiées ; revue indépendante traitée et compilation toutes cibles
RC0. [Gels et limites](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops).
Les 162 avertissements bibliothèque et 6 lib-test restent ouverts, Clippy absent.
Le propriétaire authentifié et ServiceCold ont depuis reçu le raccord ciblé
décrit ci-dessus ; la sortie explicite de quarantaine reste ouverte. Aucun
lancement, fencing, runtime ou gate global n'est qualifié par ce lot.

## 5 novembre — cohérence des faits terminaux retenus

Un contrôle pur recoupe le Worker central terminal avec l'intention, le scope,
l'admission, l'autorité et le handle retenus, les ACK Arm/WorkerClaim et la paire
coverage/result V4. Les champs immuables sont liés avant toute projection qui
recopie des données du Worker. Le Claim historique doit être exactement la
transition Store depuis son parent ; les noms sont liés au namespace exact.
Les quatre records de résultat existants sont partagés avec leur producteur,
sans changer leur format.

Le parent ciblé passe après RED initial et trois variantes supplémentaires
reproduites en revue ; compilation toutes cibles RC0. La fixture utilise un vrai
journal, HTTP et SQLite, avec moteur synthétique déclaré. Les snapshots suivis
restent inchangés pendant les contrôles purs. Revue indépendante traitée ;
[preuves et limites](VALIDATION.md#5-novembre--cohérence-pure-des-faits-terminaux-retenus).
Ces données n'authentifient pas le client et ne donnent aucun droit d'ACK ou
de lancement. Boucle froide, chemin sans blob, fencing et runtimes restent
à qualifier ; 181 avertissements bibliothèque et 8 lib-test restent ouverts.

## 5 novembre — capacité logique pour l'audit V4 retenu

La préparation interne du reader restreint conserve une seule réservation
logique par acteur, après inventaire physique complet du scope et validation
des jobs/rejets. Elle compte les futurs Cancel et un ACK sélectionné, avec
plafonds bytes/entrées/révisions et tailles imbriquées. Le drop libère la
réservation faible sans modifier le journal ; les Cancel sélectionnés et
voisins restent possibles.

Sept parents ciblés passent, dont la comparaison avec l'encodeur de production
aux largeurs maximales, la promotion réelle V1/V2 par Cancel et le refus d'un
sibling incomplet. Revue indépendante source et oracles sans blocage identifié.
[Contrat](BROKER_JOURNAL.md#préparation-bornée-de-laudit-retenu--5-novembre)
et [preuves](VALIDATION.md#5-novembre--capacité-logique-de-laudit-v4-retenu).
La réserve ne garantit ni blocs/inodes disponibles, ni terminal/authentification,
ni ACK. Le raccord au writer et à la boucle froide reste ouvert ; G09 FAIL,
aucun gate global clos et aucun moteur supplémentaire qualifié.

## 5 novembre — bornes HTTP pendant une lecture en flux

Les deux limites des premiers tests du port HTTP sont maintenant discriminées :
un Worker valide de plus de512Kio transmis sans Content-Length est refusé,
et le même budget expire pendant un corps partiel. Les cinq parents du module
passent. Deux copies privées avec une garde retirée échouent comme attendu ;
un essai ayant réutilisé le cache est écarté avant compilation en cible neuve.
[Preuves et périmètre](VALIDATION.md#5-novembre--bornes-http-en-flux-et-fraîcheur-des-preuves).
Le code applicatif ne change pas dans ce complément de tests. Ces résultats
ne mesurent pas le délai45s réel ni ne raccordent le journal ou la boucle froide.

## 5 novembre — admissions bornées du writer

Tous les producteurs du writer partagent maintenant un comptage RAII de C
admissions ordinaires. Une réservation privée non clonable peut occuper une
place supplémentaire sans élargir cette capacité ordinaire ni contourner FIFO.
Le retrait de l'enveloppe rend la capacité avant le traitement de sa valeur ;
abandon, fermeture et erreurs rendent les admissions et les entrées appropriées.
Les façades Full/Stops, la lease, les catégories NotQueued/Unknown et le délai
produit de10secondes restent conservés.

Huit parents ciblés passent, quatre nouveaux et quatre canaris du writer ;
compilation toutes cibles RC0 et revue indépendante sans défaut bloquant dans
ce périmètre.139avertissements bibliothèque et3lib-test restent visibles.
[Gels et limites](VALIDATION.md#5-novembre--admissions-raii-et-réservation-de-queue-du-writer).
La réservation est uniquement une capacité de queue : aucun budget durable,
limite de reprises actives ou ACK froid n'est encore accordé. La composition
froide, le fencing et la qualification runtime restent ouverts.

## 5 novembre — reçu HTTP exact pour l'audit retenu

Le port Rust interne lit un worker descriptif et publie uniquement les deux
kinds d'audit retenu, sur les routes versionnées existantes. Il exige un vrai
HTTP 200 et le StoredEvent complet identique à l'événement soumis : doublons,
champs inconnus, transformation, clipping et reçus réduits à une séquence sont
refusés. Une échéance monotone commune borne les échanges ; les corps et la
profondeur JSON sont limités. Aucun droit de journal, tenant ou terminal ne
découle de ces données. Le client partagé interdit les réessais implicites.

Les trois parents ciblés passent après RED réel, avec HTTP/SQLite réels pour
le périmètre A/B et le replay, et un peer adverse déclaré pour les refus.
Le canari d'autorisation existant et la compilation toutes cibles passent aussi,
avec133avertissements bibliothèque et3lib-test ; Clippy reste BLOCKED.
[Gels, commandes et limites](VALIDATION.md#5-novembre--port-http-exact-de-laudit-retenu).
Ce port n'est pas encore composé au writer ni à la boucle froide. La reprise
froide reste ouverte : son harnais distinct reproduit l'absence de publication
de deux événements V4 en mode restreint. Aucun moteur, modèle ou cluster n'est
qualifié par ce lot ; fencing, G09 et les gates globaux restent ouverts.

## 5 novembre — client broker HTTP et reprise du résultat Kubernetes

Le client Rust négocie le protocole et l'identité réels avant/après Claim,
conserve l'horloge originale et produit un ACK live privé uniquement pour
un201 exact encore valide. Le coordinateur compose cette capacité avec le
permis consommable, le transport et le journal, sans reconstruire de preuve
Store depuis JSON. Le journal vérifie le snapshot attendu complet avant ACK
durable. La capacité filesystem précède l'effet ou Result HTTP ; sa closure
conserve l'ownership après perte de son attente async.

**PASS logiciel ciblé :17fonctions uniques** sur leurs gels distingués :
2JournalIo,9client,4coordinateur distant et2canaris Store8. Le refus local
et le vrai400 central ont chacun leur RED et leur classification durable.
Une lecture HTTP d'ACK est interrompue après vrai commit central : reprise
du candidat exact, premier refus conservé, un seul événement Result et un
seul POST Kubernetes. [Preuves et limites](VALIDATION.md#5-novembre--client-broker-http-et-journal-kubernetes-composé).
La compilation composée toutes cibles passe, RC0 ; revue indépendante
source et oracles traitée, guides Kubernetes FR/EN synchronisés. Les115warnings
de bibliothèque et3lib-test restent visibles : garde sans avertissement non
satisfaite, chemins internes encore sans entrée de production.

HTTP/SQLite/fichiers sont réels, le peer TLS Kubernetes est déclaré et ne
lance aucun Pod. La reprise journal conserve le client et le contrôle vivants ;
elle ne qualifie pas un redémarrage froid. Activation de production, workspace,
sessions, cluster, fencing et pannes physiques restent ouverts. Aucun nouvel
endpoint de lancement ni `--runtime kubernetes` n'est annoncé. Clippy BLOCKED,
composant absent ; G09 FAIL,0/12 gates globaux clos, périmètre produit inchangé.

## 5 novembre — autorités distinctes dans la vue des workers

Le dashboard conserve et affiche séparément l'autorité déclarée du lancement
et celle de la dernière inspection enregistrée, avec son runtime et sa date.
Une absence reste inconnue ; aucun hôte, broker ou état courant n'est déduit.
Les identifiants sont conservés exactement, bornés et validés avant affichage.
Le même dashboard consulte plusieurs autorités d'un tenant via les contrats
existants, sans accès moteur ni nouvelle route. Guides locaux FR/EN synchronisés.

Les neuf tests du modèle frontend et le contrôle TypeScript passent. Le test
HTTP/SQLite du dashboard partagé passe avec A1/A2 chez A et B1 chez B,
headers de tenant opposés, lectures croisées refusées et POST de réservation
via le dashboard refusé405. Les six tests du fichier API concerné passent.
[Commandes, gels et limites](VALIDATION.md#5-novembre--autorités-du-lancement-et-de-linspection-dans-le-dashboard).
Il s'agit de registres de contrôle réels dans une fixture logicielle ; aucun
worker runtime n'est lancé. Build Vite et navigateur ne sont pas exécutés sur
ce delta. La fédération runtime multi-hôtes et sa qualification restent ouvertes,
ainsi que les autres exigences produit. G09 FAIL, 0/12 gates globaux clos.

## 5 novembre — décisions Kubernetes V2 et six routes centrales authentifiées

Le Store émet maintenant Admission depuis la politique serveur courante et
Reconcile depuis la vraie réservation originale. Le scope précède tout lookup
idempotent ; Reconcile n'alloue rien et ne rend jamais Fresh. Sous9, une V2
appropriée peut relire les reçus historiques V1, tandis que les capacités mémoire
V1 restent refusées. Six routes du gateway authentifient le broker et déduisent
le tenant côté serveur ; corps/réponses fermés et bornés, secrets connus criblés
avant persistance/diffusion. [Contrat API](../api.md#kubernetes-held-create-http-v1).

**PASS logiciel ciblé :11fonctions uniques**,5Store V2,3API avec vrai routeur
Axum/SQLite,2canaris legacy et1canari JSON runtime. Aucun ignoré. Les huit
nouveaux tests ont d'abord échoué avant implémentation. Un ancien oracle
incompatible avec le wrapper opaque a échoué aussi sur la base `734c78d` ;
seul ce test est corrigé, sans affaiblir le produit. [Gels et preuves](VALIDATION.md).
Compilation toutes cibles PASS RC0 ;75warnings bibliothèque et1lib-test
subsistent, donc la garde sans avertissement n'est pas satisfaite. Clippy est
BLOCKED (composant absent de l'image) et release NOT_RUN sur cette tranche.

Guides Kubernetes FR/EN et API synchronisés ; revue indépendante du code,
du correctif d'oracle et des guides favorable. Client broker HTTP, preuve ACK
live à réception, Coordinator9, activation, workspace et sessions Kubernetes
restent à implémenter/qualifier. Aucun appel cluster, E2E moteur ou fournisseur
dans cette tranche. Branche de travail non intégrée à la référence ; G09 FAIL,
0/12 gates globaux clos. Le prochain raccord porte sur le vrai client broker.

## 5 novembre — format d'audit Kubernetes 9 et sauvegardes compatibles

État historique au gel `734c78d`, avant le producteur V2 décrit ci-dessus.

La migration explicite8→9 conserve une sauvegarde8 vérifiée et les projections,
quotas, CAS et namespace. Un marqueur ordinaire fermé engage le vrai genesis8
et le head précédent ; marqueur absent, dupliqué, mal lié ou daté après sa
réception sont refusés. La lease live existante est conservée dans le même
processus ; réouvrir la base reste inactif. Aucun objet SQL supplémentaire.

**PASS logiciel ciblé :7fonctions uniques exécutées**, incluant vraie CLI
courante8→9, protocole/anciens lecteurs6/7/8 et ancien binaire refusant9 avant
mutation, avec témoin positif sur une copie7→8. Le canari ignoré par défaut
est exécuté explicitement ; [gels, RED et limites](VALIDATION.md) sont distincts.
Compilation toutes cibles PASS RC0 ;86warnings bibliothèque et1lib-test,
garde sans avertissement non satisfaite. Revue indépendante source traitée.

La validation/reconstruction V1 historique est disponible, mais mutations et
lookups/replays opérationnels Kubernetes9 restent fermés. Le producteur
EvidenceV2, la décision actuelle admission/réconciliation et l'ACK broker HTTP
sont les prochaines étapes obligatoires. Ni format9 ni configuration de
politique ne donnent de permission CREATE. Guides Kubernetes FR/EN mis à jour,
travail de branche non intégré à la référence ; G09 FAIL,0/12 gates globaux clos.

## 5 novembre — politique Kubernetes centrale chargée par tenant

Le gateway charge et canonise désormais `held_create_policy` depuis sa
configuration serveur. Validation fermée, empreinte de politique stable,
correspondance exacte autorité/image/profil et isolation des clones gateway
sont vérifiées. Une configuration invalide est refusée avant ouverture des
bases ; absence/null ne donne aucune autorité. Le fichier opérateur reste
inchangé. Les guides de configuration Kubernetes FR/EN décrivent ce champ.

**PASS logiciel ciblé :9fonctions uniques**,3politique pure,2configuration,
2état API et2canaris préflight existants ; aucun ignoré. Compilation toutes
cibles RC0. Les [gels, RED, commandes et preuves](VALIDATION.md) distinguent
chaque étape ; seuls les tests touchés sont rejoués.85warnings bibliothèque
et1lib-test subsistent : garde sans avertissement non satisfaite.

Cette tranche fournit de la donnée de configuration, sans producteur central
d'autorisation ni activation Pod. Migration9, décisions admission/réconciliation
avec politique courante, raccord broker HTTP et ses ACK live restent ouverts.
La suite conserve SQLite au gateway et credentials Kubernetes au broker,
sans nouveau service. Clippy BLOCKED (composant absent) ; release et moteurs
réels NOT_RUN sur cette tranche. Branche de travail non intégrée à la référence,
G09 FAIL et0/12 gates globaux clos ; le mandat produit complet reste actif.

## 5 novembre — refus de résultat durable et provenance vérifiée

Le Store distingue désormais, pour un résultat lié à son intention et à son
claim exacts, un epoch ou une révision réellement dépassés d'un conflit
générique. Le coordinateur retient le premier refus dans `refusal.json`,
avec le candidat/opération/CAS inchangés. Un refus différent ou un ACK reçu
plus tard ne réécrit pas ce diagnostic historique. Une erreur de vérification
d'un ancien audit devient indisponible, sans être attribuée au CAS courant ;
le Store reste invalidé et ne modifie rien.

**PASS logiciel ciblé :17fonctions uniques**,9Store,7coordinateur et1journal,
aucun ignoré ; compilation toutes cibles RC0 et revue indépendante traitée.
[RED, gels, commandes et limites](VALIDATION.md). Les données d'audit altérées
sont des fixtures privées ; les tests établissent réouverture et barrières
logicielles, pas résistance à une coupure physique ou effets Kubernetes réels.

Travail de branche non intégré à la référence. L'ancien lecteur refuse le
nouveau code fermé `control_rejected` ; les consommateurs Rust doivent gérer
les nouvelles erreurs typées. Raccord broker HTTP/Store, politique centrale
réelle, I/O sur worker borné, activation et parcours de session Kubernetes
restent ouverts.85warnings de bibliothèque et1de test : garde sans avertissement
non satisfaite ; Clippy absent et release non rejouée sur cette tranche.
Prochaine priorité : contrat et raccord opérationnel du broker, sans ouvrir
le cluster. G09 FAIL et0/12 gates globaux clos ; objectif produit inchangé.

## 5 novembre — rétention et reprise communes des résultats Kubernetes

Le coordinateur Rust local conserve maintenant le candidat exact pour une
réponse valide, une réponse incertaine et un refus avant envoi après claim.
Confirmation centrale puis ACK local durable précèdent tout handle
d'inspection. La reprise du candidat ne refait ni claim ni POST Kubernetes ;
une intention divergente est refusée. Le refus opaque d'une fenêtre expirée
reste une inconnue conservatrice, sans inventer un changement d'autorité.

**PASS logiciel ciblé :5parents, aucun ignoré**, avec Store SQLite, fichiers
et TLS locaux réels ; compilation toutes cibles RC0 et revue indépendante
traitée. [Gels, RED, commandes et limites](VALIDATION.md). Les réponses de Pod
restent des fixtures déclarées ; aucun effet cluster n'est qualifié.

**Au gel de ce premier coordinateur (`66e9dc1`).** Le refus central n'avait pas
encore de code durable dans `refusal.json` ; ce manque logiciel est traité
par la tranche décrite ci-dessus. Raccord broker HTTP/Store, I/O synchrones sur worker borné,
constructeurs d'autorité/activation et parcours de session Kubernetes restent
à implémenter ou qualifier. Le contrôle émet84warnings de bibliothèque et1de
test ; la garde sans avertissement n'est pas satisfaite. Clippy manque dans
l'image de validation, release n'a pas été rejouée pour cette tranche.
La priorité suivante est le raccord opérationnel conforme aux frontières existantes. G09 FAIL,
0/12 gates globaux clos ; le mandat produit complet est inchangé.

## 5 novembre — permission et transport CREATE vérifiés sur TLS local

La suite du même worktree non intégré ajoute une permission consommable,
liée au vrai claim central, au journal et à l'horloge prélevée avant ce claim.
Le POST collection est fixe, sans retry ; le profil201 est revalidé avant
résultat, puis l'autorité est relue. Une reprise observe l'opération existante
sans créer de claim absent. Un retour incertain garde le quota et ne produit
aucun handle. Les reçus exacts puis leur rétention durable restent requis.

Quatre parents de permission/reprise, trois parents TLS et trois canaris
existants GET/dry-run/annulation sont PASS, sur leurs gels distingués dans
[VALIDATION.md](VALIDATION.md#5-novembre--permission-et-transport-create-sur-tls-local).
La revue source indépendante ne signale pas de défaut bloquant dans ce périmètre.
La compilation toutes cibles du gel transport passe également ; des avertissements
de code inutilisé demeurent. Le coordinateur commun de rétention des résultats
incertains et de reprise d'ACK est décrit ci-dessus. Le raccord d'exploitation
et l'intégration de la verticale complète restent ouverts.

L'activation n'a toujours aucun constructeur de production ; sa couture de
test retient le listener TLS local exact. Aucun endpoint de lancement, option
`--runtime kubernetes`, appel cluster, release ou charge métier n'est ajouté.
Le passage logiciel sur un peer TLS détenu ne qualifie ni Kubernetes réel,
ni CNI/admission/NamespaceUID atomique, ni les pannes physiques. Le produit et
les gates restent ouverts, sans changement du périmètre demandé.

## 5 novembre — fondation d'admission Kubernetes hors cluster, en cours

Worktree `codex/kubernetes-held-create-20261105`, base `7155ab3` ; source non
encore intégrée. Contrats/rendu fermés, migration SQLite 7→8 avec sauvegarde,
transactions et audit/replay typés, quota partagé et journal privé sont écrits.
Trois agents se répartissent journal, tests ciblés Luna et revue indépendante ;
l'orchestrateur possède les contrats et transactions transverses.

Le dernier gel composé vérifie 15 parents Store, 7 parents FS, 3 parcours réels
Store/journal et un canari de protocole ; la compilation toutes cibles passe.
Les deux gardes de préflight schema8 passent sur leur gel précédent. Les
corrections de déduplication des protections, rétention historique et
publication dans la fenêtre sont vérifiées et relues. Des avertissements de
code encore inutilisé subsistent ; Clippy manque dans l'image de validation.
Les anciens FAIL d'oracle restent documentés : l'ouverture diagnostique d'une
base invalide est permise, avec Store unhealthy et mutations refusées.
[Résultats par gel](VALIDATION.md#5-novembre--fondation-dadmission-kubernetes-hors-cluster).

À ce gel de fondation, permission/transport CREATE n'étaient pas raccordés ; aucun appel cluster,
start/release ni session Kubernetes n'est ajouté. L'autorisation exacte n'a
pas de constructeur de production. Les observations Pod sont synthétiques,
déclarées et partielles ; elles ne qualifient pas l'admission, RBAC/CNI,
NamespaceUID, fencing, reprise réelle ou la capacité 100. Le produit complet
et ses gates restent ouverts ; G09 FAIL, 0/12 gates globaux clos.

## 5 novembre — réconciliation après annulation, périmètre logiciel vérifié

Le broker peut réconcilier le terminal du worker annulé déjà possédé, sans
nouveau start, helper ou retrait de gardien. Autorité/CID, Arm/claim historiques,
configuration et StartedAt doivent correspondre ; les inconnues conservent
le quota. La transaction Terminal enregistre l'allocation unique non facturable.
Après perte de réponse, le résultat retenu reprend sa publication idempotente.
Quatre nouveaux parents puis deux canaris PASS ; compilation toutes cibles
sans avertissement et format ciblé PASS. Revue indépendante et corrections
des guides EN/FR traitées ; code `f738662` composé en `7cdb969`, sources Rust
égales au gel vérifié. [Preuves et limites](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation).
Code et guides/preuves `2533c91` sont poussés, références distantes exactes
vérifiées. Après conservation des deux brouillons documentaires dans une archive
privée distincte, le donneur propre est retiré normalement, consommateurs finis
et relevé lsof vide ; branche distante, sources et preuves conservées.

Les nouveaux parents utilisent des observations runtime synthétiques et un
Writer de fixture à 60 s ; le produit garde 10 s, non qualifiés pour ce parcours.
Stopping reste relu, sans scénario distinct ; reprise froide, D5/fencing et
moteurs réels restent ouverts. L'optimisation envisagée du Writer est rejetée
après revue pour préserver les contrôles tardifs, sans lancer son test
exploratoire ni le pousser. QA fermée sans OOM ; preuves conservées.
G09 reste FAIL, 0/12 gates globaux clos.

## 5 novembre — documentation alignée sur les accès humains existants

Les prérequis d'exploitation pointent désormais vers les guides d'identité
locale et de SSO Web/CLI ; l'ancienne affirmation « intégration absente »
est retirée. Le périmètre UI distingue les vues techniques viewer, les
lectures nominatives et le registre brokers permissionné de la cible unifiée
complète. Les deux copies de couverture restent identiques et n'annoncent
plus ces lectures comme entièrement absentes. Sources Rust/TypeScript relues,
liens corrigés et vérifiés ; aucune nouvelle qualification SSO ou frontend.
[Résultats et limites](VALIDATION.md#5-novembre--cohérence-des-guides-et-contrats-kubernetes).

Le contrat Kubernetes retenu est précisé et ses trois écarts de revue traités,
sans code CREATE ni action cluster. Les donneurs documentaires sont intégrés,
poussés, leurs références distantes vérifiées, puis retirés normalement après
fin des consommateurs ; branches et preuves conservées. À ce gel documentaire,
la reprise d'un worker annulé était encore en validation ; le résultat logiciel
plus récent figure ci-dessus. G09 reste FAIL, 0/12 gates clos.

## 4 novembre — direction Kubernetes locale approuvée, aucune admission ajoutée

Une [proposition de création retenue](KUBERNETES_ADMISSION.md), relue
séparément, décrit permission avant CREATE, UID/spec reçus, ACK perdu,
NamespaceUID et limites de la schedulingGate. Son premier lot envisagé
n'autorise aucune libération ; le véritable adaptateur reste obligatoire.
Le mainteneur autorise conception, code Rust et tests locaux de cette direction,
sans start/release ni action cluster. Aucun cluster, droit ou runtime modifié,
aucun test runtime effectué par ce document ; 0/12 gates clos. Les travaux
indépendants de reprise Rust continuent.

## 4 novembre — autorisations broker depuis le CLI

`workload prepare/apply/resume/status` prend en charge les actions fermées
`broker_grant` et `broker_revoke`, sans clé générée, migration ni nouvelle
permission serveur. Requêtes/reçus retenus, contrôle du périmètre avant POST,
CAS, révocation, rejeu après perte d'accusé/expiration et regrant avec ancien
heartbeat : 11 fonctions uniques PASS, compilation toutes cibles et format
ciblé PASS ; revue indépendante traitée.
[Commandes, RED, résultats et limites](VALIDATION.md#4-novembre--grants-broker-durables-depuis-le-cli).
Guides et exemples EN/FR synchronisés. Les guides fournisseurs expliquent
aussi le choix API explicite après quota abonnement ; aucun appel modèle.
Code `fe3b558` composé en `0962231`, avec fichiers égaux au gel vérifié.
La documentation fournisseurs `0c4cbf1` est composée en `e6e0b25` et poussée.
Guides/preuves `b0eb665` composés en `d47b7f6`, poussés sur les deux branches
avec références distantes exactes et arbres suivis égaux. Après retour des
trois agents, état propre et relevé lsof vide, le worktree donneur est retiré
normalement ; sa branche et les preuves privées restent conservées. Le
conteneur QA est clos, sans changement applicatif après les vérifications.

Ce lot complète le provisionnement opérateur du registre, sans fournir le
transport worker, la fédération de jobs, Kubernetes ou une qualification
runtime. L'investigation distincte de reprise d'un worker annulé échoue
encore dans le setup de son harnais, avant l'oracle ; code applicatif inchangé
et preuves conservées, travail indépendant poursuivi. G09 reste FAIL,
0/12 gates clos ; ancien artefact V1 et panne disque non qualifiés ici.

## 4 novembre — build refusé avant moteur si le paquet est incomplet

`build --source` vérifie désormais 7 répertoires et 19 fichiers requis avant
de sélectionner ou contacter le moteur. Absences, liens détectés et types
incorrects produisent un diagnostic relatif borné. Les quatre canaris
d'identité/endpoint/arguments restent exercés avec des fixtures adaptées.
Sept fonctions ciblées et compilation toutes cibles PASS après RED réel ;
[preuves et limites](VALIDATION.md#4-novembre--prérequis-de-build-avant-tout-appel-moteur).
Les guides locaux EN/FR expliquent le refus et l'action de reprise.
Revue indépendante traitée ; code `7406e4f` composé en `fb918c9`, sans
différence applicative supplémentaire et avec sources égales au gel testé.
Guides/preuves `91de7c7` composés en `c51be48`, poussés et références
distantes conformes. Donneur propre retiré normalement après relevé des
consommateurs ; branches, gels et preuves privées conservés.

Ce préflight ne valide ni contenu, ni totalité des imports, ni efficacité
de `.dockerignore`, contexte figé ou atomicité des images. Aucun build
d'image réel n'est exécuté par cette validation. G12, reprise, relais,
fédération et Kubernetes restent ouverts ; G09 FAIL, aucun gate clos.

## 4 novembre — canal de relais étudié, primitives Docker observées

Le relais HTTP/SQLite et ses identités existent, sans raccord depuis les
workers sans réseau. La [proposition de canal versionné](DECISIONS.md#4-novembre--proposition-de-transport-des-workers-sans-réseau-décision-attendue)
précise ownership exclusif, wrapper/ENTRYPOINT, durabilité et terminaison
distante ; elle attend une décision avant code. Deux probes OS sur Docker
Desktop transmettent exactement stdin/NUL/EOF et une réponse avant EOF,
sans TTY distant, réseau ou credential. Le conteneur jetable est fermé.
[Preuves et limites](VALIDATION.md#4-novembre--primitives-docker-du-futur-canal-de-relais).

Aucun worker produit n'est lancé. Ce constat ne qualifie ni canal,
handoff, arrêt/reprise, Podman ou Kubernetes ; G09 FAIL, aucun gate clos.
Les guides de tenancy et de recette distinguent désormais les contrats
backend livrés du transport manquant, sans déclarer à nouveau OIDC absent.
Proposition/preuves `5f853f3` intégrées en `a8c16e0` et poussées, références
distantes conformes. Demande de direction soumise au mainteneur ; aucun
accord déduit du silence. Donneur propre retiré normalement, branches et
preuves conservées ; travaux indépendants poursuivis.

## 4 novembre — diagnostic d'un journal de récupération absent

Le broker distingue un historique requis absent d'une entrée invalide :
`needs_reconciliation` remplace le refus trompeur sur les ouvertures sans
création. Aucun inventaire vide, nouveau journal ou droit d'exécution n'en
est déduit. Les UUID nuls de Cancel restent refusés après authentification,
avant ouverture. Les guides opérateur EN/FR indiquent les preuves à préserver.
[Vérification et limites](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).
Sept fonctions ciblées et compilation toutes cibles PASS, revue traitée ;
code `3f8b33d` intégré en `0281bb1`, sources égales au gel vérifié.
Guides/preuves `6f7d398` composés en `36d9c39`, poussés avec références
distantes conformes. Donneur propre retiré normalement après contrôle des
consommateurs ; branches, gels et preuves privés conservés.

La reprise D5, le transport des workers, la fédération multi-hôtes,
Kubernetes et les qualifications réelles restent ouverts. G09 demeure FAIL ;
aucun gate global n'est clos par ce correctif de diagnostic.

## 4 novembre — coût mesuré et relecture redondante retirée

Les synchronisations des ancêtres, absentes du compteur de test, sont
désormais comptées au scope appelant. Le RED mesure1 au lieu6, puis les
deux consommateurs concurrents passent. Une mesure manuelle réelle Store/
HTTP/Stream passe à N0/8/32 ; un second RED constate16fsync pour une simple
lecture du snapshot. Le getter devient `cached_head`, sans autorité de POST,
en conservant root/lock/anchor ; append, préPOST, ACK et reopen conservent
leurs vérifications complètes. Un historique altéré peut provoquer les GET
identity/self avant refus, mais aucun nouveau POST.

Après modification, le cycle à32 opérations historiques passe de262 à198
sync fichiers et de831 à634 sync répertoires. Ce sont des comptes sur
tmpfs LinuxARM64, pas un gain de latence disque ou un SLO Mac. Sept parents
uniques ciblés passent sur les gels reliés, dont le parent manuel exécuté
explicitement avant/après ; all-targets final PASS. Aucun worker/modèle
n'est lancé. [Détail et limites](VALIDATION.md#4-novembre--compteurs-de-durabilité-et-snapshot-du-publisher).

La complexité reste croissante avec l'historique : trois scans complets
par cycle neuf sans retry et un clone d'identités O(N). Grande rétention, pannes
physiques, capacité100 et qualification runtime restent ouvertes. G09 FAIL,
aucun gate global clos ; Clippy Linux BLOCKED. Codec9f5d49 intégré6af1081,
guides/preuves0416db4 poussés et références distantes vérifiées ; donneur
retiré normalement après inventaire et contrôle des consommateurs. Branches,
gels et notes privés préservés ; checkout d'intégration maintenu.

## 4 novembre — publisher possédé par le service broker

Le raccord Rust de `broker serve` utilise une clé Broker distincte et les
trois routes existantes identity/self/heartbeat. Son activation par profil
wire2 est explicite ; état privé, pending/reçu durables et acteurs possédés
par le service. Les guides [EN](../setup/broker/PRESENCE.md)/[FR](../setup/broker/PRESENCE.fr.md)
et l'exemple typé couvrent autorité, fichiers privés, reprise et capacité.

21 fonctions uniques passent sur les gels reliés :19 bibliothèque et2
parcours de vraie CLI Full/Recovery, puis all-targets sur le gel final.
Le dernier parcours prépare le journal via List authentifié, ferme le
helper Full vide, possède son enfant StopsOnly et exige un vrai RPC avec
la configuration présence absente. Les FAIL des fixtures/protocoles et
leurs corrections restent consignés. Aucun job ou worker n'est lancé.
[Résultats et limites](VALIDATION.md#4-novembre--présence-émise-par-le-service-broker).
Code8ead8cc intégré2949b54, guides/preuves8d9abc7 poussés et références
distantes vérifiées. Donneur propre sans consommateur retiré normalement ;
branche, gels et preuves privés conservés, checkout d'intégration maintenu.

Les RED ont aussi vérifié deux défauts de capacité/historique et la
séparation publication/admission avant leurs corrections. Panne de flux
seule ne tue pas les jobs ; matériel privé dangereux ferme l'admission.
Coût des quatre scans par cycle, rétention, fautes physiques, reprise D5,
transports workers, fédération et qualifications runtime restent ouverts.
Clippy Linux BLOCKED ; G09 FAIL, aucun gate global clos.

## 4 novembre — portée courante authentifiée du broker

Le nouveau `GET /api/v1/brokers/identity` lie la même preuve Broker au tenant,
namespace et révisions courantes du serveur, via une seule transaction
contrôlée. Une collision de métadonnée avec un secret refuse toute la réponse.
Six fonctions ciblées et all-targets PASS, revue traitée, guides API/EN/FR
synchronisés ; [RED, OOM de compilation et reprise](VALIDATION.md#4-novembre--identité-broker-liée-au-même-périmètre-serveur)
conservés séparément. Code intégré en87e902d depuis le donneurfc22ca9,
worktree donneur retiré normalement, branches/preuves préservées.

Cette lecture seule ne publie pas la présence de `broker serve` (raccord
distinct ci-dessus) et ne fournit ni santé runtime, inventaire d'hôtes ni
autorité pour un POST futur. Transports, fédération et qualification restent ouverts.
Clippy Linux est BLOCKED ; aucun gate global clos, G09 reste FAIL.

## 4 novembre — cible Kubernetes locale préparée en lecture

Docker Desktop expose le cluster local1.36.1, un nœud Ready et environ12Go
de RAM, sans pression signalée au relevé. Deux profils publics et guides
EN/FR séparent inspection et création Pod. Le seul profil lecture est créé
dans un Namespace neuf, sans remplacer de ressource. Ses sept décisions
RBAC sous impersonation passent : deux lectures Namespace autorisées,
lecture default/list global/Secrets/Pod-create/exec refusés. Les quatre
objets sont conservés pour poursuivre l'essai local ; aucun Pod ni token
n'est créé. [Commandes, résultats et limites](VALIDATION.md#4-novembre--préparation-rbac-kubernetes-docker-desktop).

Ce contrôle opérateur ne teste pas encore doctor/préflight, une authentification
ServiceAccount réelle, le runtime Pod ou l'isolation CNI. Ces qualifications
restent NOT_RUN, sans blocage cloud artificiel ni fermeture d'un gate.

## 4 novembre — opérations CLI des identités techniques

Le client Rust workload fournit préparation privée sans HTTP, émission/
rotation/révocation avec le runner courant, reprise exacte et consultation
locale. Le scope attendu est fixé avant toute mutation ; le tenant reste
authentifié côté serveur. La clé et l'intent sont resynchronisés en reprise,
les secrets contrôlés avant échappement JSON et les reçus publiés sans
remplacement. Un reçu historique ne prouve jamais l'activité actuelle.

**17 fonctions uniques ciblées PASS**, build release et all-targets PASS,
revue indépendante traitée ; les RED et cinq échecs de fixture intermédiaires
sont conservés. Le Worker est vérifié sur projection logicielle autorisée
et API réelle, sans container. Guides EN/FR et exemples sont fournis.
Le complément vérifie expiry réelle, reprise historique, conflit CAS et
capacité d'audit indisponible, sans replay des tests inchangés.
[Gels, commandes et limites](VALIDATION.md#4-novembre--cli-durable-des-identités-techniques).
Clippy Linux reste BLOCKED ; publication interrompue par faute réelle,
handoff, transport worker, fédération et reprise runtime restent ouverts.
Aucun nouveau service, rôle, endpoint ou stockage ajouté ; aucun gate clos.

## 4 novembre — parcours natif SSO vérifié dans son périmètre Linux

La recette H contre un vrai Keycloak local neuf passe sur5f508cf :
Code/PKCE/callback, admission, identité/contexte, deux lectures avec reçus,
refresh/génération suivante/contexte renouvelé, logout confirmé côté
fournisseur et contrôle, sorties CLI/gateway et audits durables vérifiés.
Treize phases, un scénario réel ;17tests purs du harnais passent avant lui.
Les résultats et FAIL historiques restent distincts. Le contrôle ferme
sa session et trace l'opération ; la CLI possède son appel fournisseur.
[Preuves, sources et fermeture des acteurs](VALIDATION.md#4-novembre--sso-cli-réel-renouvellement-et-déconnexion).
Ce résultat LinuxARM64 isolé ne qualifie ni navigateur composé, CLI macOS,
TTY, entreprise/TLS, révocation globale des JWT, worker ou autre runtime.
La matrice SSO, D5, relais, fédération et Kubernetes complet restent ouverts.

## 4 novembre — déconnexion OIDC explicite

Le protocole d'accusé configuré Web/CLI est intégré dans la branche de
validation par1b2d34f, avec27fonctions produit ciblées PASS, contrôle de
toutes les cibles et revue indépendante. Les champs restent facultatifs ;
le comportement historique est conservé en leur absence. Les profils et
guides EN/FR suivent dans5f508cf, avec revue indépendante ; trois profils
validés par Rust, build et all-targets composés passent. La préparation
neuve hors ligne passe via stdin ; le mode fichier échoue RC1 sans cause
établie et reste à diagnostiquer, sans effacer cet échec par le succès stdin.
Le cookie quoté du harnais privé passe11tests après un RED synthétique.
Le FAIL réel G reste conservé ; aucun succès IdP/logout n'est déduit de
ces seuls tests. La recette H ci-dessus apporte séparément la preuve réelle
du refresh et de la rotation du contexte. [Preuves et limites](VALIDATION.md#4-novembre--accusé-de-déconnexion-oidc-et-profils-explicites).
Les gates globaux restent ouverts, G09 FAIL et0/12 clos.

## 4 novembre — découverte des jobs en récupération

Le lot `RecoveryInventory` est intégré dans la branche de validation par
`3acd5cc` (donneur `1df111e`). La CLI découvre les UUID/head confirmés sans
ouvrir les blobs métier et réutilise le reader, la queue et les planchers STOP.
L'identité runner/tenant/namespace/profil est revalidée avant accès au journal.
Un profil froid d'un service normal refuse sans ouvrir ni mettre le service
en maintenance ; la découverte froide exige le mode récupération explicite.
Les guides EN/FR couvrent commande, pagination, états partiels et limites.

**24 fonctions Rust uniques ciblées PASS** sur les gels reliés, dont14nouvelles ;
`cargo check --all-targets` PASS sur le gel final. Les deux erreurs de
fixtures puis le stack overflow de matrice restent conservés ; leurs
corrections sont limitées aux tests. Revue indépendante traitée sans
blocage statique. L'inventaire ne sonde pas le runtime, ne prouve aucun arrêt,
ne renouvelle aucun délai et ne libère aucun quota. Il ne clôture ni D5 ni G09.
[Commandes, gels et limites](VALIDATION.md#4-novembre--inventaire-stop-du-broker).

Le préflight Kubernetes avec CLI et audit réel est intégré par96bf8ed :
24tests logiciels ciblés et all-targets passent, revue indépendante sans
blocage statique, guides et exemples EN/FR livrés. La compilation de toutes
les cibles de la composition broker+Kubernetes passe également sur96bf8ed.
Aucun appel cluster ; le lancement de pods reste indisponible.
Le diagnostic privé SSO G passe ses sept tests mais sa scène réelle refuse
`cookie_value` avant callback. Cette garde composée doit être précisée hors
ligne à ce gel ; le complément courant ci-dessus reproduit et corrige la
grammaire quotée hors ligne, sans nouveau résultat réel de logout.
Les prochaines étapes sont ces validations puis le lancement/reprise réels,
le relais runtime et la fédération multi-hôtes ; aucun gate global clos.

## Vue humaine des brokers — composition partiellement validée

Backend Rust et frontend TypeScript ont été développés depuis `5b6dfc1`
dans deux worktrees distincts. La capacité explicite `read_broker_registry`, la
migration7 et la vue humaine sont composées sur la branche de validation ;
elles ne sont pas reportées au checkout principal/protégé et leur parcours
Web complet reste à qualifier. Les checkpoints Rust `d3baf0c` et frontend `514ca12` sont
composés avec les douze guides/index EN/FR sur la branche de validation
`codex/human-brokers-validation-20261104`, à `9ee05cc`, poussée. Luna a vérifié
les42hashes source et12hashes documentaires, sans rejouer les tests. Les deux
worktrees donneurs sont retirés normalement après contrôle des consommateurs ;
branches/proofs préservées. Le worktree de validation conserve les sources
pour les contrôles manquants. L'ancien worktree d'audit a retrouvé son état
propre ; aucun checkout protégé n'est modifié.

Sur les gels backend successifs : **64 fonctions Rust uniques ciblées PASS**.
Le gel final stage7 passe aussi `cargo check --all-targets`, sans warning.
Deux oracles ont été corrigés pour vérifier le refus effectif d'un Store
diagnostique invalide, et un import manquant des tests unitaires est rétabli ;
aucune correction produit n'est alléguée pour ces erreurs de test. Les échecs
et les filtres sélectionnant zéro test sont préservés séparément, sans les
compter comme succès. Clippy Linux reste `BLOCKED`, outil absent.

Sur le gel frontend stage6 : **35 tests Node PASS** ; TypeScript a passé sur
le code applicatif byte-identique du stage5. La revue a fait corriger un
verrou de budget levé avant validation complète de la réponse. Les nouveaux
scénarios couvrent aussi vide/indisponible/dense et réponses tardives après
perte d'identité/expiration/logout, mais restent non exécutés au navigateur.
Build Vite, navigateur et parcours Web composé réels restent `NOT_RUN` :
les outils Linux manquants ont une préparation de cache épinglé soumise au
mainteneur. Ces résultats ne qualifient ni la fédération multi-hôtes, ni des
workers réels. [Preuves et limites](VALIDATION.md#4-novembre--vue-humaine-des-brokers-gels-en-cours).

Trois essais SSO CLI réels D/E/F ont été exécutés indépendamment des outils
frontend absents, sur Linux ARM64/Docker et un Keycloak local neuf isolé.
F passe compilation et six tests ciblés après un RED Version1 confirmé ;
le correctif du seul harnais n'accepte que cette métadonnée connue, sans
ouvrir les autres attributs, domaines, chemins ou limites. Le scénario F
valide le vrai Code/PKCE et callback, l'admission, l'identité personnelle,
la sélection d'un contexte offert et la lecture autorisée avec reçu.
Il échoue ensuite sur le guard composé `logout_contract` ; les champs de
déconnexion fautifs restent inconnus, et la vérification finale des audits
n'est pas atteinte. La fermeture du parcours complet n'est pas qualifiée.

Le profil loopback canonique et ses deux guides EN/FR, relus indépendamment,
sont intégrés par `44cb51e` dans la branche de validation poussée. Les trois
hashes correspondent à la source testée `ad22d33` ; seules les preuves
engineering diffèrent. Son worktree donneur est retiré normalement après
contrôle des consommateurs, sans effacer branches ni preuves. Les gels D/E
et leurs échecs antérieurs restent conservés. La prochaine étape est un
diagnostic fermé à critères inchangés, avant toute correction produit.
Le dernier gel G s'arrête plus tôt sur le parseur de cookie du harnais ;
les observations F restent attachées à leur gel. Aucun navigateur/macOS natif/SSO entreprise ni worker
réel n'est qualifié par ce scénario.
[Preuves et limites](VALIDATION.md#4-novembre--prétests-du-driver-sso-cli-privé).
Le DNS hôte Keycloak est revalidé : le blocage historique est levé, sans
preuve de CLI natif macOS. WSI-Q02 reste ouvert : l'analyse confirme des
revalidations complètes et propose une mesure à compteurs avant optimisation,
sans garantie de coût global ni capacité100 déduite.

## Correctifs initiaux de l'audit en cours

Les lots export A, disponibilité multi-tenant B et admission ingress C sont
intégrés et poussés sur `codex/audit-integration-20261103`, jusqu'à `2eeaa0d`.
Le lot E est intégré dans `808f08c` et sa clarification API dans `14d5eb8` :
les tentatives de connexion sans preuve vérifiée ne consomment plus le journal
durable partagé. Sept régressions et sept canaris OIDC passent au premier gel ;
le dernier oracle renforcé passe séparément après modification de sa fixture,
avec production inchangée. Guides EN/FR et revue indépendante acceptés.
Le lot D est intégré dans `b168784`, issu de `3820bec` : I/O SQLite HTTP
hors Tokio, permis du store réellement consulté, révalidation au point d'I/O
et cleanup borné. Revue finale et 59 fonctions uniques ciblées PASS.
Le contrôle composé de toutes les cibles passe également, sans warning,
sur une nouvelle cible privée ; aucun test exécuté par ce `cargo check`.
Reproductions comportementales avant correction, tests ciblés et revues
indépendantes sont consignés dans [AUDIT_FOLLOWUP.md](AUDIT_FOLLOWUP.md).
Sur leurs gels respectifs : 26 fonctions Rust ciblées PASS, trois canaris
model_ingress locaux supplémentaires PASS, 24 tests frontend et TypeScript
PASS. Cela ne constitue ni une suite complète ni une qualification runtime.

Le contrôle composé `cargo check --offline --locked --all-targets` passe sur
`2eeaa0d`. Deux avertissements de fixture propres à Linux sont corrigés sans
changer ses cas ; le contrôle ciblé correspondant passe sans avertissement.
Les preuves, sources et limites sont dans [VALIDATION.md](VALIDATION.md#3-novembre--correctifs-audit-abc-et-compilation-composée).
Clippy Linux reste `NOT_RUN` : composant absent de l'image, préparation
épinglée soumise au mainteneur. Aucun test worker réel n'est ajouté par ces lots.

Les worktrees A/B/C/D/E/F1/SSE sont fermés après intégration et contrôle
des consommateurs ; preuves et références source sont préservées. Les RED de
D ont précédé ses corrections, puis les unités/canaris et la revue finale.
F a son RED sur D intégré : baseline16, 48 lectures acceptées, head64 ; les
contrôles et le second humain A échouent ensuite, B reste disponible.
La conception de budgets/réserves est relue ; les deux implémentations ont été
menées dans des worktrees distincts, avec tests et revue indépendants.
Le sous-lot SSE est désormais intégré dans `297376f` (source `279b5f2`) :
4 flux par principal canonique, 24 humains dans les 32 du tenant, permis
conservés jusqu'au drop du Body, même terminal/EOF. Huit fonctions ciblées
passent après RED et revue indépendante.
Les budgets durables sont maintenant intégrés dans `aa4eef9` (source
`4095586`) : 21 nouvelles fonctions et 30 canaris passent après reproduction,
correction et revue. Deux régressions du candidat issues de revue ont été
reproduites séparément (initialisation partielle et mapper générique), puis
corrigées et vérifiées. F1 refuse A1 au head23/429 tout en conservant
décision, ingestion, lecture d'A2 et révocation, avec B200. Sur l'archive
composée `aa4eef9`, cinq tests du raccord F1/SSE et la compilation de toutes
les cibles passent sans warning. Les gates globaux restent distincts.
Le client Web conserve désormais leurs codes/actions de reprise (`f5b035a`,
14 fonctions ciblées PASS après RED/revue). Ce test client ne prouve pas
l'émission backend du budget durable, prouvée séparément par le test F1.
Le risque FD a son premier RED Store réel sur `a1fb518` : après restitution
des descripteurs, chaîne valide mais autorité restant interdite. Le rejeu sur
`b168784` confirme le même défaut. Son sous-agent de conception a été arrêté
par la demande d'authentification matérielle Daybreak de cette session ;
compilation, documentation et tâches indépendantes continuent.
Le journal SSO déjà
plein et le TTL initial de bout en bout restent des écarts distincts.
L'audit externe reste ouvert ; ses nouveaux verdicts sont rapprochés des
instantanés et du candidat dans le suivi, sans qualification globale.
G09 reste FAIL et **0/12 gates globaux sont fermés**.

Un raccord fonctionnel postérieur est corrigé dans `99c2ce2` : la maintenance
humaine hors ligne accepte désormais une base6 validée inactive, après RED
réel du garde4/5 périmé. Quatre régressions CLI/SQLite, un canari historique4/5
et all-targets passent ; revue indépendante et guides EN/FR synchronisés.
Les barrières tenant/filesystem/audit/confirmation/inactivité sont conservées,
sans activer la supervision ni migrer implicitement. [Preuves et limites](VALIDATION.md#4-novembre--maintenance-humaine-après-migration-workload6).
Le proxy dashboard conserve aussi le refus F1 durable (`167ff85`) après
RED réel gateway429/proxy503, puis trois régressions et un canari de contexte
PASS, all-targets sans warning et revue indépendante. Réponse locale fermée
sans corps/headers amont ; aucune modification des contrôles d'identité.
[Preuves et limites](VALIDATION.md#4-novembre--refus-f1-conservé-par-le-proxy-du-dashboard).

## Chantiers parallèles après les correctifs F

Les trois chantiers partis de `2c40097` ont leurs propriétaires distincts.
Le backend composé est intégré par `c762a24` (source `567a673`) : **86 tests
uniques ciblés PASS**, canari de révocation composé PASS et compilation de
toutes les cibles sans avertissement. Ces résultats et les limites sont
décrits dans [VALIDATION](VALIDATION.md#4-novembre--backend-workload-registre-et-relais-intégré).

- `kubernetes-adapter-20261103` : connexion HTTPS explicite, diagnostic et
  observation liée aux UID. **11 tests parents PASS** : 8 HTTP/TLS/config,
  puis 3 sur mTLS, fermeture des connexions à l'annulation et bornes du corps.
  Helper interne non recompté. Les deux revues indépendantes ne signalent
  aucun défaut bloquant dans ce sous-lot readonly ; deux anciens échecs du
  harnais restent conservés. [Guide EN/FR](../setup/kubernetes/README.fr.md)
  et [preuves](VALIDATION.md#3-novembre--diagnostic-kubernetes-et-durée-de-vie-du-transport).
  La commande `kubernetes doctor` est intégrée par `bebdd90` : quatre tests
  CLI/TLS ciblés et all-targets PASS, revue indépendante acceptée, guides EN/FR
  à jour. [Preuves propres au CLI](VALIDATION.md#4-novembre--commande-cli-de-diagnostic-kubernetes).
  Aucun CREATE/STOP, lancement de
  pod, exec/attach ou confinement Kubernetes n'est qualifié. L'inventaire
  local read-only Docker Desktop montre kindnet ; la présence d'une API
  NetworkPolicy ne prouve pas son application.
- `worker-relay-20261103` : contrats, transactions SQLite, grants dirigés,
  ACK/révocation, quotas et modules HTTP composés avec le registre. Tests
  fonctionnels, bord de canonisation temporelle, replay après révocation,
  requêtes indexées, plancher UTC et revalidation des preuves humaines passent
  sur le gel testé ; les constats bloquants des revues ont été corrigés.
  Transport Unix/runtime et recette de trois workers réels restent ouverts.
- `broker-registry-20261103` : registre, incarnations, ownership des autorités
  et bindings workload digest-only, migration6, replay, budgets workload et
  raccord CLI/configuration sont vérifiés dans le périmètre logiciel indiqué.
  Les gels historiques, défauts de fixtures et deux attentes HTTP corrigées
  restent consignés. La validation finale travaille directement sur source
  sous montage OS readonly, avec nouvelle compilation au changement de gel.
  Aucune fédération distante qualifiée.

Revues du raccord : refus manquant de collision humain/workload, association
des nouveaux index relay au schéma exact et compatibilité du client broker6
ont été identifiés, reproduits, corrigés et relus. Le schéma JSON publié accepte
désormais les versions réellement servies2 à6, dont3 à6 actives ; un test dédié
compare les réponses Router et les contraintes publiées. Les campagnes RED
SQL/UTC, puis GREEN, ont leurs preuves sous montage source RO. Coût historique des reconstructions complètes après
commits de connexions sœurs et mutations de supervision : **WSI-Q02 ouvert**,
aucune garantie de coût global borné ou de capacité100. Les guides opérateur
EN/FR documentent la distinction entre ce backend et le transport worker.
Depuis `5b6dfc1`, deux implémentations distinctes commencent la lecture humaine
du registre : backend Rust/schema7 et modèle/client/vue TypeScript, avec Luna
pour leurs tests ciblés séparés. Le contrat relu impose un droit explicite
non délégué aux admins d'organisation et le parcours broker seul, sans
élargir les permissions existantes. Code de cette nouvelle vue, recette UI
et schéma7 restent à livrer/valider ; aucune disponibilité n'est présumée.
Les worktrees donneurs du registre et du relais sont fermés après contrôle
des sources intégrées, références poussées et consommateurs accessibles.
Le brouillon antérieur du relais reste conservé sous une référence Git dédiée ;
aucune preuve, donnée ou branche n'a été supprimée. Le worktree donneur du CLI
Kubernetes est également fermé après intégration et contrôle ; son diagnostic
n'autorise aucun pod.
Le candidat reste non qualifié production : reprise D5, vrais workers,
transport, fédération, Kubernetes et les autres gates conservent leurs écarts.

Root possède les contrats et la composition. Le propriétaire du relais écrit
les nouveaux modules HTTP ; celui du registre raccorde CLI/configuration.
Les producteurs utilisent des budgets workload sous les
réserves contrôle/identité de F1 : un worker ne doit pas pouvoir consommer la
réserve contrôle en étant classé comme une écriture de confiance. Les tests
Luna et les revues sont parallélisés avec le code ; une seule compilation
lourde est admise. Les worktrees D5 existants restent hors de ces périmètres.

## Historique du 2 novembre — D5 et SSO

Dernier contrôle logiciel D5 : les 37 fonctions des fondations et les 31 fonctions composées précédentes passent dans leurs gels ciblés. Le FAIL acteur antérieur reste non reproduit, de cause indéterminée. Le nouveau test du pump **FAIL** sur le gel `Efq7q2` : dépôt du signal dans l'outbox, `JournalError::Io` après 10003 ms ; le second test est **NOT_RUN**. Compilation toutes cibles et harness **PASS** pour ce même gel. Le diagnostic du writer continue sans augmenter les délais. Les nettoyages de code et le nouveau test de saturation ont passé une revue de source ; leurs tests compilés restent à exécuter. Aucun moteur ni parcours vertical complet n'est qualifié. [Résultats et limites](VALIDATION.md#2-novembre--diagnostic-ciblé-du-writer-d5).

## Préparation SSO locale — intégrée, recette complète ouverte

**DNS hôte résolu le 2 novembre :** après autorisation et authentification administrateur du mainteneur, `/etc/hosts` contient `127.0.0.1 keycloak.ai-guard.test`. Résolution native macOS et discovery de l'issuer exact, avec Authorization Code et PKCE S256, **PASS**. Les mentions du blocage DNS dans les résultats antérieurs ci-dessous sont historiques. La prochaine qualification CLI exige encore un binaire natif de provenance actuelle, puis un login et des lectures réels ; les anciens binaires de récupération ne prouvent pas ce parcours. [Preuves et limites](VALIDATION.md#2-novembre--dns-hôte-keycloak-résolu).

Le lot `7a08d18`, publié sur `codex/sso-setup`, est intégré par `586c17e`. Le CLI Rust prépare hors ligne une destination privée neuve : bases tenant migrées5 avec sauvegardes, journal d'authentification distinct, credentials techniques et memberships OIDC explicites. La reprise conserve identités et reçus ; elle ne fabrique ni ressource ni session. Profils Compose autonomes Docker/Podman, manifeste public et guides EN/FR sont livrés. [Contrat et résultats](LOCAL_SSO_SETUP.md).

Les8tests CLI setup et24tests audit ont leurs résumés PASS ; leur code de sortie Cargo final n'a pas été enregistré. Les4unités setup finales,2canaris fichiers corrigés, fmt et Clippy strict ont chacun RC0 et sources stables. Revues correctives traitées. L'image de contrôle6040058a est construite depuis d343848 ; un vrai volume neuf est préparé sous1000:1000, répertoire700/fichiers600. Gateway, dashboard et ingress démarrent sans réinitialisation et leurs endpoints publics répondent avec le bon issuer. Ces recettes utilisent des CIDR QA explicites, pas une qualification des pools standards. [Preuves](VALIDATION.md#2-novembre--installation-conteneurisée-sso-neuve).

Le défaut d'appel au fetcher natif découvert par Chrome est corrigé et publié0ddd585 : RED ciblé,7tests pertinents, build TypeScript/Vite et revue traitée. Son imagead2e9c1f est réellement déployée sur les trois services, volume et Keycloak inchangés. DNS hôte manquant nécessite une intervention administrateur pour le CLI natif. L'ancien Keycloak et ses données restent actifs/conservés ; seul son ingress est arrêté réversiblement pour libérer18080. Aucun worker ni modèle lancé par ce lot ; G09 reste FAIL,0/12gates clos.

Le navigateur atteignait page d'entrée et formulaire IdP, puis callback303→complete403 ; aucun POSTtokens. Trois régressions dans `tests/oidc_gateway.rs` reproduisent le refus de la page publique :1RED403au lieu200 et2canarisPASS. Un premier filtre erroné avait sélectionné0test, NOT_RUN conservé. Correctif limité à cette navigation publique exacte GET sansquery/navigate/document, SSOconfiguré ; revue sécurité approuvée,6tests du fichier/fmt/Clippy strict PASS avec RC0 et sources stables. Source API SHA `fb5a3b393c87fe3b1200edc5cf685e0f3f3aec8800dce6eee1d73b950d86935b`, test SHA `9e99c1e48e6c3c917abdf1d9d5500fea8db2a874fc71de8c776c9d5ed3e95ac5`. Un lint du harness non publié a été corrigé séparément, sans refaire les6tests inchangés. Ces résultats logiciels précèdent la nouvelle image et ne qualifient pas rétroactivement les anciens parcours échoués.

Le correctif publié `96fe9be` est désormais construit et déployé sur gateway/dashboard/ingress : image `sha256:72bfb9119fe552c6100ffaeb07cbbd8bd86f9449270b19f3904e97ca2dd8c5c7`, build et rollout RC0, trois services healthy/1000:1000/read-only/CapDropALL/no-new-privileges, volume et setup conservés sans réinitialisation. Vrais parcours Keycloak Web : `user-1` **PASS 7/7, RC0**, `user-2` **PASS 8/8** dans sa recette de rôles, puis `admin-ai-guard` **PASS 13/13, RC0** dans une recette distincte sur la même image. Les trois comptes ont une preuve de connexion, identité, lecture autorisée et logout ; la réponse administrative HTTP200 contient zéro ressource, avec contrat/reçu validés et UI vide cohérente. Les trois échecs administratifs antérieurs restent FAIL : causes fines du premier `empty=false` et du deuxième `identity_context` inconnues, troisième capture manquant une vraie requête car son prédicat exigeait à tort `view=metadata`. Seul ce dernier défaut du harness est démontré et corrigé, sans changement produit. Le mapping DNS appartient uniquement au Chromium dédié ; CLI natif **BLOCKED DNS hôte/intervention administrateur**. Refus adverses de permissions, refresh/révocation, délégation et qualification SSO globale restent ouverts. [Preuves et limites](VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée). Aucun worker/modèle lancé, G09 FAIL, 0/12 gates clos.

Diagnostic broker distinct : les tests restent en précondition avant l'oracle, dans le seed Worker, après180s en debug. Ownership parent des deux Child et conservation des preuves corrigés/revus ; aucune optimisation métier ni durée Arm modifiée. Un passage release depuis la source privée immuable `ai-guard-preclaim-release-input-20261102.QICMdw` a été interrompu avant le test pour donner priorité au défaut SSO : cible/cache conservés, NOT_RUN, aucun effet runtime. Les neuf fichiers de harness restent non publiés et ne sont pas dans les images SSO. Le poste observé a8CPU,0%idle et forte charge ; les compilations lourdes restent séquentielles, analyses/revues/tests légers indépendants parallèles.

Complément courant : la reprise release a d'abord échoué en compilation faute d'un JSON de fixture dans l'archive, zéro test/NOT_RUN. Un nouveau gel complet de248entrées a ensuite compilé et exécuté une seule fonction : FAIL RC101 avant l'oracle temporel,46,84s, erreur d'`arm_and_claim` sans le type `EnforcerStageFailure` attendu. Sources inchangées, enfants terminés, preuves conservées ; aucun RED sur les quatre étapes ni effet runtime démontré. D5-U avance séparément dans le worktree `enforcer-full3` : records et socle FS/ledger écrits, join/protocole en cours. Première compilation debug du gel D5 PASS, puis19tests records exécutés :17PASS/2FAIL sur le refus d'extensions imbriquées, correction en cours. Une revue a aussi repéré un conflit injustifié entre scopes dans l'unicité des retraits ; sa régression précède la correction. Aucun lot D5 intégré ni gate clos ; [preuves et limites](VALIDATION.md#2-novembre--canary-préclaim-et-fondations-d5-en-cours).

## G09 — essai courant refusé avant lancement

Release de test d6cb136 compilée (RC0, aucun test exécuté par le build), puis une unique recette Docker réelle : FAIL275,75s à `readiness_before_claim`. Worker Created, aucun Running/claim/fault SIGKILL ; oracle de deadline NOT_RUN. À la première vérification Ready, le slot est stopping et son horloge prouve l'expiration de l'Arm5s depuis20,609s. Cause dominante de latence encore non mesurée : pending journal → POST central → ACK journal → register/reopen. Instrumenter avant d'optimiser, sans nouveau budget ni retrait de durabilité. [Preuves et portée](VALIDATION.md#2-novembre--g09-release-courant-refus-avant-claim).

Processus12284, worker/volume/racine privée et release f798bcb2 conservés. Ne pas reconstruire cette cible release tant que le gardien/les ancres la retiennent ; debug distinct utilisable. Le worktree `codex/sso-setup` est intégré, sauvegardé et retiré normalement après contrôles clean/ancestor/absence de consommateurs ; sa branche locale/distante et toutes les preuves restent conservées. Gouvernance du checkout principal et anciens gardiens inchangés.

## SSO Web/CLI — lots parallèles en intégration

Recette humaine locale réelle Docker Desktop/LinuxARM64 : API/CLI A/B, permissions distinctes, exports neufs, migrations sauvegardées3→4→5, redémarrage et révocation hors ligne PASS sur l'image `954246…051e` du commit7afdd43. Audits finaux valides (A31/B20événements), SQLite5/integrity_check=ok ; volumes et preuves conservés. Navigateur NOT_RUN : module du runtime historique manquant, Playwright local absent, puis hôte d'accès navigateur CUA indisponible lors du diagnostic direct. Aucun credential transmis aux outils. Cette recette ne lance aucun worker et ne remplace pas A1/A2/B1. [Preuve](VALIDATION.md#2-novembre--recette-humaine-réelle-docker-apicli-ab).

L'orchestrateur intègre les lots Web, CLI et audit avec propriétaires exclusifs ; Luna conserve une seule compilation lourde à la fois. Le frontend intégré passe63tests et son build sous Node25.6.1. Sur les gels documentés du2novembre : humain local14/14, journal d'authentification24/24, OIDC désactivé3/3, flow gateway7/7 et plafond de permissions22/22 PASS. Le test de transport préparatoire passe après correction de sa fixture ; les15autres résultats antérieurs sont conservés sans rejeu global. Les24tests CLI finaux passent ensemble, notamment les trois nettoyages d'admission/rotation, SIGINT, expiration, masquage et refus de tuples mal liés. Fmt et Clippy strict finaux PASS. Les correctifs et guides/configurations EN/FR/API ont leur revue indépendante sans blocage restant identifié dans ce périmètre. Ni Keycloak réel, ni navigateur, bootstrap complet, délégation ou fédération ne sont qualifiés. [Preuves et limites](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues).

Le candidat4b525e4 est poussé, référence distante exacte vérifiée. Les quatre donneurs OIDC sont sauvegardés sur leurs branches distantes et en archives privées, puis retirés normalement après inventaire et contrôle des consommateurs. Seuls le checkout principal, `broker-service` actif et `broker-supervision` retenu pour ses anciens gardiens restent enregistrés. Nouveau worker générique Docker construit/inspecté, pas encore lancé ; G09 reste FAIL. [Image et nettoyage ciblé](VALIDATION.md#2-novembre--image-worker-générique-et-nettoyage-ciblé).

## Image de contrôle — permissions publiques vérifiées

Après le FAIL de politique0600 illisible, le commit publié `7afdd43` normalise uniquement les assets publics. Sa nouvelle image Linux/ARM64 est construite depuis148fichiers Git vérifiés, avec un contexte toujours0600 ; les étapes Rust/frontend restent en cache. Un vrai probe UID1000, sans réseau/montage, rootfs en lecture seule, capabilities supprimées et no-new-privileges lit politique/front/favicon/licence. `/data` et `/run/secrets` restent0700, UID1000. PASS packaging ciblé, puis recette humaine A/B ci-dessus. [Preuve et chronologie](VALIDATION.md#2-novembre--image-corrigée-lecture-réelle-par-uid1000).

Ce fichier rassemble les constats datés et leurs limites. Chaque résultat reste attaché au gel, au checkout et à l'environnement nommés ; il ne qualifie pas automatiquement la branche courante, un autre worktree ou la pile QA. Aucun commit initial n'existait au début du chantier ; les fichiers préexistants étaient non suivis et le travail a commencé en place avec périmètres exclusifs. Le30octobre, à la demande du mainteneur, le checkpoint initial `fa75d6ef34ee506a4d5facdc9e50fc515f79cf7b` est poussé sur `origin/main` du dépôt privé configuré, référence distante vérifiée. Secrets, `.env`, caches, bases et volumes restent hors Git ; seules les preuves assainies de documentation sont partagées. Branches et worktrees sont désormais possibles. La copie initiale `/tmp/ai-guard-baseline.KqN9Hi` reste une archive temporaire, pas une release.

## Lots parallèles ouverts — identité, dashboard et registre

Le1er novembre, trois worktrees distincts partent du checkpoint publié `2bb6acb` : `identity-sso`, `dashboard-unified` et `broker-registry`. Le domaine humain, ses permissions et la migration explicite3→4 ont depuis leur validation logicielle ci-dessous ; le dashboard lié est publié sous `48ae2a6`, sans inventaire complet déduit d'une page. Le contrat broker reste un cadrage avant raccordement. `authority_id` reste une identité de moteur, jamais une identité de broker. Aucune connexion SSO ni fédération multi-hôtes n'est livrée par ces lots.

Le lot TLS à volumes neufs reste distinct : **R3 logiciel PASS**, dix unités actives, CLI inchangé, fmt et Clippy strict, revues traitées ; release native et image Linux/arm64 composées cd3d0b5 PASS. Import réel Docker Desktop et diagnostic des modes PASS ; parcours HTTPS gateway→relais PASS, panne avec refus durable et reprise santé PASS. **L'unique tentative de décision modèle reste FAIL**, refus tracé `transport_unavailable`, cause fine non établie, usage inconnu ; aucun retry ni délai augmenté. Les cinq nouveaux services sont arrêtés et leurs données/preuves conservées. Le contrat du registre broker est conservé et son worktree de cadrage fermé proprement sans changement produit ; l'implémentation attend le raccordement d'identité. Les agents de tests Luna et les revues prennent le relais des implémenteurs ; trois sous-agents simultanés au maximum. Les propriétaires suivent chaque worktree jusqu'à validation, intégration/push vérifié et inventaire avant retrait ; les preuves privées et les consommateurs vivants sont préservés. [Preuves et limites TLS](VALIDATION.md#1er-novembre--import-tls-vers-volumes-validation-logicielle-r3).

## Diagnostic Jev — validation logicielle ciblée

Le lot distinct de diagnostic Jev est **PASS logiciel ciblé** :24 fonctions, fmt/Clippy strict et revues traitées. Les échecs de transport portent des observations fermées par requête, sans détail sensible ; une collision de masque avec la trame locale fixe refuse avant POST. Contrat public/downcast conservés, paramètres/fingerprint explicites, aucun délai/retry changé. [Preuves](VALIDATION.md#1er-novembre--diagnostic-jev-fermé-et-trame-locale). Ce résultat n'explique ni ne remplace le FAIL_MODEL réel précédent.

## Identité humaine — domaine et stockage vérifiés

Socle Rust distinct des clés techniques : principaux locaux/OIDC typés, comptes, digests de credentials, memberships tenant/organisation, capacités et grants de ressources. Le parsing d'un principal n'authentifie personne. Projections indexées du schéma4 exact, mutations/audit atomiques, CAS/replay et scopes revalidés avant accès. Un humain ne peut pas s'attribuer une ressource historique non liée ; premier binding opérateur obligatoire, organisation conservée après tombstone, refus identique connu/inconnu. Migration3→4 explicite et sauvegardée, CLI `migrate-control --target-version 2|3|4|5`, défaut2 conservé ; anciennes données/namespace/claims et inactivité à réouverture préservés.

**Logiciel PASS sur les gels documentés :** sept unités uniques, onze intégrations identité/migration, canaris pertinents et quatre contrats API, soit26 fonctions uniques ; fmt et Clippy strict. Le premier test P1 était RED avant correction ; un échec de compilation SQL et un canari aux faux tokens présents dans ses hashes sont conservés séparément. Revue indépendante traitée, procédures EN/FR mises à jour. [Commandes, preuves et limites](VALIDATION.md#1er-novembre--domaine-identité-et-migration4).

Ce socle est complété par le parcours local ci-dessous. **Release native arm64 PASS** sur65f828a : trois binaires construits sans exécution,215 entrées suivies Rust/Cargo/toolchain inchangées. Le wrapper initial échoue après la fin de Cargo ; une vérification en cache du même helper et des mêmes sources confirme RC0 en0,80s. **ABSENT / NOT_RUN :** login OIDC Web/CLI, délégation de lancement, nouvelle image humaine5 et parcours navigateur contre cette image. L'image Linux/arm64 du précédent cd3d0b5 ne qualifie pas ce code humain nouveau ; ni compilation ni migration ne clôturent SSO/G04/G08.

Les worktrees de cadrage broker, TLS et dashboard précédents sont fermés normalement. Les donneurs `identity-sso` et `human-dashboard` sont également fermés après sauvegardes poussées/vérifiées sous `14bce708` et `c5c116c4`, équivalence aux sources composées66c1bcd, scans propres et inventaires sans consommateur. Les sorties frontend ignorées sont conservées hors checkout par renommage sur le même filesystem, inodes/métadonnées préservés ; branches, archives et preuves restent disponibles. Après publication65f828a, `jev-diagnostics` est fermé normalement : ses six sources sont archivées, sa branche de sauvegarde5cfc904 poussée/vérifiée, puis son checkout réconcilié sur le commit publié et retiré proprement. Le contrôle négatif des consommateurs reste limité au périmètre lsof/config accessible. `broker-supervision` reste conservé pour ses sept gardiens ; aucun n'est arrêté ou désarmé par ces fermetures.

## Accès humain local — validation logicielle ciblée

Authentification locale nominative, provisioning/reprise CLI, API humaines versionnées, consultation metadata/contenu/export et dashboard distinct du viewer sont implémentés. Les droits viennent des memberships/grants durables ; premier binding de ressource opérateur et fait connu obligatoire. Le schéma5 ajoute l'audit nominatif des lectures via migration4→5 explicite sauvegardée ; `migrate-control --target-version` accepte2/3/4/5, défaut2 inchangé. Aucune réactivation implicite ni rôle runner donné à un humain.

Contextes opaques, révision globale privée, curseurs AEAD et limites strictes permettent les précédents/retries et la reprise SSE après frame complet. Chaque lot est audité avant restitution ; il ne prouve pas réception/téléchargement. La révocation ne retire pas les octets déjà émis. Les erreurs401 retirent la session humaine même après changement de contexte ; refus ciblé, contexte expiré et panne de service ont leurs reprises distinctes.

**PASS dans les gels documentés :**7 unités contexte,18 contrats API/SSE (dont70batches avec reprise et nouvelles arrivées),4 migrations,14 parcours CLI,2 masquages et4 canaris exacts. Les préparations/style incorrects détectés pendant les premières passes sont corrigés sans affaiblir le produit ; les échecs restent conservés. Fmt/Clippy strict finaux PASS, revues indépendantes traitées. Front composé :17 unités/build PASS, régressions401/focus et changement humain/viewer vérifiés sur leurs gels, style OVPT conservé. [Commandes, chronologie et limites](VALIDATION.md#1er-novembre--accès-humain-local-apicli-et-dashboard).

Ces tests emploient de vrais Store/SQLite/routeurs/CLI locaux ; les trois identités A1/A2/B1 ne sont pas la recette de trois workers Docker. Le navigateur utilise des fixtures dans ses tests. Node24 unités, image humaine5 et raccord Web au vrai contrôle conteneurisé restent NOT_RUN ; OIDC, lancement délégué, fédération et cloud restent ouverts. Guides utilisateur/opérateur et API EN/FR synchronisés. G09 FAIL et aucun gate global clos.

## Dashboard — navigation liée vérifiée

Liens bidirectionnels entre sandboxes, agents et workers depuis les API réelles existantes, avec chargement/pagination du registre, provenance, fraîcheur et reprises. Les identifiants reçus restent des corrélations, pas des preuves d'exécution ou d'identité broker. Revue indépendante traitée : registre périmé depuis Allocations et focus perdu reproduits par trois scénarios RED puis GREEN ; un bandeau d'erreur résolu découvert sur les captures est également corrigé après son propre RED. Build,32 unités ciblées et parcours navigateur passent sur leurs gels respectifs ; la dernière correction ne rejoue que build et scénario concerné. [Preuves et limites](VALIDATION.md#1er-novembre--navigation-liée-du-dashboard-et-reprises).

Captures de fixtures à1440/390/320px en clair/sombre, deux relues visuellement par l'orchestrateur. Node25.6.1 et Chrome for Testing148 ont réellement servi ; Node24, données de moteur et déploiement de ce nouveau bundle restent NOT_RUN. Aucun SSO, humain/admin, broker distant ou capacité100 n'est déduit de ce lot ; G08 reste ouvert.

## S0 — durée de vie du verrou gardien, correctif natif vérifié

Un groupe natif ciblé reproduit la reprise du même flock après abandon du listener alors qu'un handler IPC authentifié ou un acteur natif reste vivant. Contrôles initiaux d'exclusion et trois setups réussis ; compilation PASS, groupe RED0,94s. Aucun moteur, Ready, effet runtime ou quiescence inventé : l'acteur est retenu avant sa réouverture. Le cas idle libère normalement le verrou. Ce résultat a conduit au correctif R2 ci-dessous ; D5/retrait et G09 restent ouverts. [Preuves et limites](VALIDATION.md#1er-novembre--s0-red-natif-de-durée-de-vie-de-lease).

Le candidat R2 conserve la même lease et les handles natifs via un propriétaire indépendant ; progression Weak, arrêt puis vrai join sans attendre les STOP. **Sept fonctions ciblées PASS** : parent RED inchangé, trois cas owner et trois canaris natifs du client/arrêt/reprise/Cancel. Fmt, Clippy strict all-targets et release arm64 PASS, revue source favorable. Les états inconnus restent retenus jusqu'à fin de processus, sans retrait/capacité libérée ni reçu de quiescence. Aucune qualification Docker/Podman, crash du processus ou ledger n'est déduite. [Validation R2](VALIDATION.md#1er-novembre--s0-r2-green-ciblé).

## Qualification Docker du juge TLS — démarrage en échec, 1er novembre

**FAIL au démarrage**, sur les images Linux ARM64 contrôle `5fa8a095…` et modèle `d6bdfcbe…`, dans deux projets Compose neufs et isolés. Le modèle et le relais quittent avec code1, sans OOM ; la gateway refuse sa configuration Jev et sa boucle de redémarrage est arrêtée. Dashboard et relais de publication restent Created. Une reproduction attachée du modèle établit un refus de lecture de `/models/model.gguf` ; le relais donne encore une erreur TLS générique. Les projections ont les modes/propriétaires attendus, mais cela ne qualifie pas les contrôles filesystem/ACL du lecteur Rust. Le diagnostic continue sans affaiblir les permissions, TLS ou les limites.

**NOT_RUN :** healthcheck HTTPS, décision API, inférence, scénario de panne/reprise et tout worker. Aucun appel modèle n'a eu lieu. Les cinq conteneurs initiaux, volume d'audit, PKI et preuves sont conservés ; seuls deux conteneurs stat inertes possédés ont été retirés après leurs captures respectives (prérequis puis diagnostic). Les PASS logiciels précédents gardent leur périmètre ; aucun gate fermé. [Preuves et commandes](VALIDATION.md#1er-novembre--démarrage-réel-du-juge-tls-fail).

**Correction du chemin modèle : PASS démarrage/santé isolés.** Le répertoire était root:root0444, sans traversée pour UID65532. Le seul changement Dockerfile lui donne0555, poids0444 et limites inchangés. Une image ARM64 neuve charge le modèle avec la limite1Go configurée et répond au healthcheck, réseau none et sans génération ; échantillon mémoire734,6MiB, pas un pic. Le worktree dédié suit intégration/push puis fermeture. Les fichiers TLS projetés portent la magic filesystem `6a656a63`, non autorisée par le lecteur strict : cause du refus confirmée, ACL non qualifiées, aucune extension de l'allowlist. TLS/gateway restent ouverts. [Validation ciblée](VALIDATION.md#1er-novembre--correction-de-traversée-du-répertoire-modèle).

Correctif intégré par fast-forward dans `8f6bddf`, poussé et SHA distant vérifié. Donneur propre, aucun ignoré/non-suivi ni consommateur visible au contrôle extérieur final, retrait Git normal RC0 ; branche et preuves privées conservées. Il reste le checkout utilisateur, PRIMARY actif et le worktree des anciens gardiens. Aucun volume/image/credential supprimé par cette clôture.

## Récupération STOP3 et worktrees — 1er novembre

Après R4c et ses quatre RED conservés, le candidat1 corrige ancres, forme V1 embarquée, UUID de préface3 et cap Watch. Revue source favorable et **32 fonctions ciblées PASS**, dont21 V3 et canaris2/R8/R9, fmt et compilation. Les nouveaux fichiers sont dans le manifeste ; la collecte R4b antérieure incomplète et son échec restent distincts. Clippy/release passent ensuite sur le checkpoint composé D5/TLS T2 ; aucune qualification moteur ou ledger n'est déduite du transport. [Preuves](VALIDATION.md#1er-novembre--d5-candidat1-green-et-deux-défauts-tls-reproduits).

Le vieux worktree Keycloak est fermé sans perte : cinq variantes/essais locaux sauvegardés avec hashes, trois originaux déplacés hors Git et cache target complet conservé dans un dossier privé. Seules les quatre configurations déjà intégrées servent au fast-forward ; aucune dépendance de démo ni test auxiliaire ajouté/poussé. Donneur propre, aucun consommateur visible, retrait Git normal RC0 ; branche conservée à7390e0d. Après publication des lots ci-dessous, les donneurs D5 et TLS sont également fermés ; `broker-supervision` reste retenu par ses gardiens. Ceci ne qualifie ni SSO ni un runtime.

TLS T2 corrige les deux défauts reproduits de PEM imbriqué et de lifetime après abandon : sept vérifications ciblées PASS, revue traitée, fmt/Clippy strict et release passent sur le checkpoint composé D5/TLS. Les34 PASS initiaux et deux RED gardent leurs propres gels. Un conteneur stat confirme seulement le prérequis de projection UID1000/0444 sur Docker Desktop ; la nouvelle image et le parcours TLS réel restent en qualification distincte. Le modèle reste consultatif, aucun gate fermé. [Preuves et limites](VALIDATION.md#1er-novembre--tls-t2-validation-logicielle-finale-et-prérequis-de-montage).

**Publication :** D5a334b36 puis TLS766b512 poussés, référence distante766 exacte. Les deux donneurs atteignent ce commit proprement, leurs38 chemins applicatifs sont identiques ; retrait Git normal après inventaire zéro non-suivi/ignoré/consommateur visible, branches et preuves conservées. Il reste seulement le checkout utilisateur, PRIMARY et le worktree retenu par les anciens gardiens. Une nouvelle image LinuxARM64/Node24 `5fa8a095…` est construite depuis le contexte fermé exact, sans remplacer QA. Le profil TLS réel est préparé dans un dossier privé ; aucun appel modèle n'est encore compté PASS.

## Moteurs Jev et masquage d'audit — 1er novembre

Le lot logiciel J3b est implémenté dans PRIMARY et vérifié sur ses sources gelées : sélection explicite TypeSafe, catalogue Rust et avis local Chat Completions ; priorité CLI/environnement ; provenance et empreinte non secrète ; entrées/sorties bornées ; erreurs fermées. La politique Rust obligatoire précède chaque juge. Le catalogue reste incomplet et ne reconnaît le bootstrap géré que structurellement. Le modèle local n'est pas qualifié à autoriser : son Allow devient Review, donc refus effectif.

Cinq régressions de défaut passent après leurs RED conservés : provenance absente, requête trop grande après masque, champ sûr amputé, doublons de décision/score/usage et masquage backend absent de l'API d'ingestion. L'API et les Stores qui lui sont attachés utilisent maintenant l'union immuable des secrets, via le mécanisme sibling existant qui conserve backing et origine de supervision ; aucun dictionnaire partagé mutable ni nouvelle autorité. Le Store d'origine de l'appelant reste immuable et les anciens événements ne sont ni remasqués ni réécrits. Le résultat est vérifié dans les réponses, la persistance, l'historique et les replays des fixtures concernées.

**PASS logiciel ciblé :62 fonctions**, formatage, Clippy strict all-targets et une compilation release, avec revue indépendante traitée. La première sélection de deux tests CLI avait exécuté zéro fonction ; elle n'est pas comptée PASS. Seuls ces deux filtres corrigés sont rejoués, sans recompilation ni reprise des60 précédentes. Les sources/fixtures/lock et trois binaires release exacts sont conservés hors checkout. [Commandes, preuves et limites](VALIDATION.md#1er-novembre--jev-j2j3b-corrections-et-validation-ciblée).

**Intégration et fermeture :** commit `58309ed765cabf35af0b0eada8ac53e3a0fe1c46` poussé sur `codex/broker-service`, SHA distant vérifié. Les15 contenus identiques sont indexés dans le donneur, puis son fast-forward vers ce commit réussit sans reset/force. Inventaire propre, aucun ignoré/non-suivi ni consommateur visible au contrôle final ; retrait normal Git RC0, branche conservée. Preuves et artefacts restent hors checkout. Un nouveau worktree TLS part de58309ed et possède son périmètre ; les anciens gardiens interdisent toujours le retrait de `broker-supervision`.

Les guides EN/FR, contrat API, exemple local-rules et surcharge Compose évoluent ensemble. Le raccord HTTP hôte du juge et le loopback du gateway conteneurisé restent distincts ; la liaison conteneurisée HTTPS/AC privée est le chantier suivant. Aucun appel réel TypeSafe/Qwen/Codex/Claude, moteur, SSO ou matrice100 n'est qualifié par ces fixtures. Le modèle historique reste non qualifié sémantiquement ; G09 FAIL, D5 ouvert,0/12 gates clos.

## Transport d'arrêt restreint — chantier en validation, 1er novembre

Le lot logiciel R8 est intégré et poussé dans `codex/broker-service`, commit `9054183c5e06b5b1eb491fdff98f62e27de7ddda`, référence distante identique vérifiée. Le worktree est propre à ce checkpoint ; les travaux D5 suivants sont isolés et leurs résultats ne lui sont pas attribués.

Le checkpoint WIP R5 `5f77f160f0fc5de33412a766e52474ecb51f2f36`, poussé sur `codex/broker-service` avec référence distante vérifiée, reste conservé. Ses 24 fichiers reprennent les sources exactes du gel R5 et leurs statuts ; les gels suivants sont séparés ci-dessous. Contrôle avant ce push : aucun binaire, chemin interdit, motif suspect ou secret connu détecté. Cette sauvegarde n'est ni une release ni une validation supplémentaire du comportement.

Le gel logiciel R1 ajoute une voie explicite RecoveryV2/StopsOnly, distincte du Full legacy, ainsi que `broker serve --recovery-only`. Compilation macOS native et cinq fonctions ciblées PASS : authentification/rôle, refus des commandes Full, challenge et restauration de deux cibles avec vrais Journal/Writer/IPC après remplacement du seul child de test. Ces deux cibles ne constituent pas deux tenants, et les runtimes volontairement indisponibles ne prouvent aucun arrêt moteur. Après le RED FS-01, le retrait des seules trois bornes de longueur du transport passe le test sparse au gel R2 ; les contrôles structurels restent conservés.

Les quatre défauts R1 sont corrigés au gel R3 : retry dans le même epoch, relance du thread, ancien Cancel valide après un plus récent et factory lazy restreinte. Les six RED R2 deviennent GREEN après correction et renforcement causal des oracles. Les tests de liaison MAC, Watch et reprise du transport passent aussi, soit neuf fonctions ciblées R3 ; formatage PASS. R4 reproduit la course d'epoch ; R5 la corrige par version d'observation et passe le même test, avec revue sans nouveau bloqueur dans ce delta.

Le P2 multi-Arm R3-01 est reproduit par les deux scénarios R6c, projection et reprise du journal : après confirmation réelle du Cancel et abandon du waiter pendant A tenue, B ne progresse pas. Les deux mêmes tests passent au gel R7 après rétention groupée possédée, respectivement en 88,18 s et 90,25 s, puis huit canaris couplés passent. Aucun oracle ni délai produit n'est changé. Les échecs antérieurs de PATH et de marqueur partiellement écrit restent des défauts de harnais, sans faux RED. La revue R7 ne trouve aucun défaut applicatif confirmé ; elle recommande un test supplémentaire du résultat public lors d'un changement d'epoch/révision entre les confirmations de A et B. Clippy R7 refuse une enum volumineuse et deux conditions imbriquées ; les tests CLI et la release sont donc NOT_RUN à ce gel. R8 ajoute seulement une indirection privée et les conditions équivalentes : quatre canaris, deux intégrations CLI natives, fmt, Clippy strict et compilation release passent, avec revue traitée. Trois exécutables release et les sources exactes sont conservés et rehashés ; aucun déploiement ni effet moteur n'est déduit de ce lot logiciel.

Le silence Watch est borné par le timeout de trame de 3 s, distinct des 750 ms de stagnation sur frames reçues ; la concurrence CAS réelle reste à compléter dans les tests. Ledger/retraite/V5, drainage d'un ancien start et qualification moteur demeurent ouverts. Le contrat de retrait est consolidé et relu dans BROKER_SUPERVISION.md, sans capacités fictives livrées. Les commandes, gels et limites sont dans [VALIDATION.md](VALIDATION.md). G09 FAIL, D5 ouvert, aucun gate fermé.

La couture suivante conserve les échanges2 exacts et prévoit une négociation3 authentifiée avant tout accord de rôle. Après correction de deux défauts de cleanup avant lancement, le premier test R9 atteint réellement son oracle RED : le serveur courant ne fournit pas Challenge3 après Negotiate3. Les positifs Journal/Cancel/Restore2 et le nettoyage du child possédé précèdent cet oracle. Le brouillon coverage3-sur-Status2 n'a jamais été exécuté ; aucune modification applicative, négociation3 complète ou ledger n'est livrée par cette reproduction. La qualification D5 reste ouverte.

## Dashboard — refus de session, 1er novembre

Le correctif `d1164bf4d9673be7c518c47687ab1e769341f66d` est intégré dans `codex/broker-service`. Chaque réponse401/403 du client actif ferme désormais la session avant les gardes d'affichage : un500 simultané ne masque plus le refus, un détail déjà fermé ne peut plus l'ignorer, et une ancienne session ne déconnecte pas la suivante. Le rappel SSE historique reste compatible, sans double notification. Aucun style, thème, endpoint ni droit serveur n'est modifié.

Trois RED logiciels sont conservés : API, notification SSE et navigateur sur ancien bundle. Sur le gel corrigé, onze tests API, build et navigateur ciblé PASS ; après intégration sans conflit, les onze tests API et le build repassent. Revue indépendante sans défaut confirmé. Node25.6.1, npm11.13.0 et Chrome156/Playwright1.65.0-alpha ont réellement servi ; Node24 reste NOT_RUN. Le navigateur emploie des fixtures HTTP explicites, sans qualifier l'authentification Rust, OIDC ou les workers. [Détails et limites](VALIDATION.md#1er-novembre--dashboard-refus-de-session-et-intégration).

Le cycle complet des worktrees est désormais une règle explicite d'AGENTS.md. Pour ce lot, intégration, validation ciblée et push de `d1164bf` sont vérifiés. Le checkout `dashboard-session-isolation` est retiré par `git worktree remove`, sans force, après inventaire propre : aucun fichier non suivi non ignoré,367 artefacts générés identifiés, preuves/captures conservées hors checkout et aucun consommateur visible. La branche reste disponible. Les worktrees actifs ou consommés par un gardien ne sont pas des caches jetables : `broker-supervision` reste notamment retenu par sept processus utilisant encore son binaire.

### Cycle des worktrees — inventaire du 1er novembre

| Checkout secondaire | État et condition de fermeture |
|---|---|
| `dashboard-session-isolation` | Fermé : correctif intégré, tests ciblés/revue/push vérifiés, preuves et branche conservées |
| `docker-launch-profile` | Fermé : ses19 contenus correspondent exactement au lot déjà intégré `0f7c403` ; sauvegarde de branche `427c9ba` poussée/vérifiée avant retrait Git propre |
| `jev-backends` | Actif : implémentation des trois backends et corrections des RED Jev ; validation/intégration encore ouvertes |
| `d5-ledger-red` | Actif : delta R9 présent aussi dans PRIMARY mais non committé ; D5 non livré |
| `keycloak-sso` | À réconcilier avant fermeture : variantes Cargo/README et deux tests auxiliaires à conserver hors distribution ; aucune suppression autorisée par leur seul statut ignoré |
| `broker-supervision` | Retenu : sept gardiens vivants utilisent des anciens inodes de son binaire et leurs ancres ; le binaire courant n'est pas une sauvegarde de ces exécutables chargés |

`broker-service` reste le checkout de contrôle actif ; le checkout principal et ses brouillons utilisateur restent intacts. Les fermetures n'arrêtent aucun gardien, ne retirent aucun volume et ne changent aucun statut de qualification.

## Authentification du gardien — 1er novembre

Le client lit désormais l'authentification IPC existante avant toute tentative de bootstrap. Si elle manque et que la racine contient une histoire de rôle, la connexion refuse de générer un nouveau secret. Les erreurs de fichier/permissions restent bloquantes ; l'inventaire de présence utilise un curseur indépendant. Le bootstrap legacy d'une racine vide et la lecture sans réécriture sont conservés. RED réel sur une lease noyau occupée, puis 2/2 tests ciblés et le test public avec vrai gardien vide, arrêt et libération de lease PASS ; formatage/Clippy strict PASS et revue sans bloqueur. La perte cohérente de toute la racine reste indiscernable d'une racine neuve par cette API legacy.

Au gel de ce correctif d'authentification, le premier test StopsOnly de vraie trame Unix était RED, protocole2 absent, et exclu de ce commit. Le chantier ci-dessus possède depuis ses preuves distinctes. Aucun arrêt de worker, D5 ou gate n'est qualifié par le bootstrap d'un gardien vide. La suite conserve l'ordre par Arm et le refus d'un ancien reçu lorsque l'absence de STOP intervenu ne peut être prouvée.

## Diagnostic CLI du contrôle — 1er novembre

`broker control-info --gateway ORIGIN --token-file FILE` lit désormais les métadonnées authentifiées du contrôle sans registre ni broker démarré. Il restitue tenant, rôle, namespace et état typés, y compris le schéma2 inactif avec son namespace existant. Fichier privé revalidé, requête/réponse bornées, refus des collisions de credential avant sortie ; aucune activation ou attribution humaine SSO n'est créée. Après le RED de sous-commande absente, le premier 4 PASS/1 FAIL révèle une assertion de test incorrecte sur le namespace schéma2 ; elle seule est corrigée. Résultat final : **5/5 PASS en 4,32 s**, formatage et Clippy strict PASS, revue indépendante sans bloqueur restant, guides broker anglais/français synchronisés. Les deux échecs et leurs limites sont conservés dans [VALIDATION.md](VALIDATION.md). Aucun moteur ni gate de production n'est qualifié par ce diagnostic.

## Manuel et configurations — lot publié du 1er novembre

Le manuel utilisateur adopte le contrat du produit complet, en anglais et français. Cinq guides de contexte, leurs index, six exemples publics `.env.exemple.*`, les configurations typées et les options image/ports des Compose sont publiés sur `codex/broker-service`, commit `752f3b55f7e6e62248ca663697b5a54d913a1b49`, référence distante vérifiée. Le profil Keycloak K6 est importé sans son harnais et un Compose/Dockerfile local d'inférence est distribué. Config, image et parcours applicatifs gardent leurs preuves distinctes ; cette publication ne qualifie pas leur assemblage complet.

Contrôles documentaires et de configuration : cinq guides de contexte en deux langues, index et entrée bilingues ;177 liens de16documents sans cible locale manquante. Deux revues croisées ont corrigé la valeur par défaut de sortie Qwen, l'exception des ports Keycloak et le prérequis modèle du parcours abonnement Codex. Luna a résolu13 variantes Compose5.0.2, dont surcharges de ports et vérification loopback/références ; aucune création de service ni inférence lors de ce contrôle. Le parse Compose Podman n'est pas une qualification de Podman. L'index multi-architecture llama.cpp `4b7be037…` est épinglé avec ses enfants AMD64/ARM64. Un build réel unique du nouveau Dockerfile passe ensuite sur Linux ARM64 : checksum des poids vérifié par BuildKit, image `sha256:d6bdfcbe064f5e2ef5fac7d8b143b6a7e81037e46c7efa9516e18a6cc482d8ce`, tag neuf conservé. Aucun lancement ni appel modèle dans ce build ; les autres architectures et le parcours applicatif restent non qualifiés.

Écarts code/contrat conservés : identité locale nominative et permissions personnelles ; handlers et parcours OIDC Web/CLI ; configuration typée d'identité/inférence et transports TLS/egress correspondants ; adaptation Codex/Claude au modèle interne ; sélection commune des trois moteurs Jev ; agrégation distante et Kubernetes ; points de contrôle signés/reprise qualifiée. Le manuel complet ne ferme aucun de ces chantiers. Le nom de source Claude documenté `claude_oauth_env_file` est corrigé après un RED natif ciblé ; l'ancien `claude_o_auth_env_file` reste un alias, sans réécriture des fichiers. Les dix fonctions de configuration passent ensuite sur la source corrigée, compilée dans une cible privée neuve. Formatage et Clippy strict passent également. Les deux échecs Cargo antérieurs sur une bibliothèque sans `TrustedExecutable`, pourtant public dans les sources courantes, restent conservés : cause initiale non attribuée, cache partagé intact ; le contrôle isolé n'établit pas rétrospectivement cette provenance.

Les essais auxiliaires Keycloak/Qwen/AC ne sont plus destinés à Git.114 fichiers précédemment suivis sont désindexés dans `752f3b5` ; toutes leurs copies locales sont présentes, sans réécriture d'historique. Les README de preuves gardent des synthèses et les commits historiques. Le manuel n'annonce pas ces instruments ignorés comme commandes disponibles dans un clone neuf. Les régressions du produit restent suivies.

Estimation de développement communiquée au mainteneur le 1er novembre : **environ 60 %, incertitude ±10 points**, jugement d'ingénierie sans formule ni promesse de délai. Les exigences complètes restent celles d'AGENTS.md ; SSO applicatif, reprise qualifiée, relais métier, agrégation distante et Kubernetes restent des chantiers importants. La qualification conserve 0/12 gates clos.

## Dernier lot vérifié — générateur PKI local

Le [générateur de certificats de test](../demo-pki.md) est implémenté en Rust : sortie neuve privée, AC racine/intermédiaire et chaîne serveur, exécutable de confiance revalidé, aucune réparation de l'existant ni modification de confiance/DNS. [Trois fonctions uniques et vraie connexion TLS](evidence/2026-11-01/demo-pki/README.md) passent, ainsi que fmt/Clippy strict. Le test a aligné hors ligne une seule arête du lock, sans nouveau paquet/version. Quatre FAIL de leurs gels restent conservés avec leurs diagnostics ; aucune qualification production/SSO/G11 n'en découle.

163 contenus source concordent ; les tar originaux privés comportent aussi des métadonnées AppleDouble non manifestées. Une copie portable distincte est vérifiée indépendamment :163 membres réguliers, mêmes noms et bytes, sans AppleDouble/PAX/liens, sans réécrire l'historique. Build release et autres plateformes non exécutés pour cet outil. **G09 reste FAIL, D5 ouvert et0/12 gates clos.**

## Lot précédent — journal et contrats canoniques

Le [diagnostic sur copie privée du graphe riche](evidence/2026-11-01/journal-cost/README.md#s7--mesure-sur-copie-privée-du-graphe-riche) passe :31 révisions, aucun moteur ou modèle nouveau, origine sans demande de sync/création, vrai Writer et vrai vérificateur en mémoire. Les temps sont muraux debug, sans exclusivité de charge ni preuve de latence release ou de cause H10. La factorisation canonique V2/V3/V4 conserve contrôles et formats ; trois empreintes publiques observées avant correction sont figées dans les tests.

Le [premier contrôle](evidence/2026-11-01/journal-cost/README.md#correction-canonique--premier-contrôle) et sa [reproduction instrumentée](evidence/2026-11-01/journal-cost/README.md#diagnostic-à-budgets-inchangés) conservent deux FAIL : premier `Io` non attribué ; reproduction montrant un timeout caller puis une réussite backend tardive, sans réponse livrée. Les marqueurs sont retirés exactement. Seul le test fonctionnel512 attend désormais60s, plafond déjà permis par Writer ; défaut10s et assertions restent ailleurs, aucun délai produit changé. **Résultat final : neuf fonctions uniques PASS**, dont timeout/ownership et le probe sur copie, fmt/Clippy strict/release RC0,160 sources et binaires vérifiés, revue traitée. Les [mesures après](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot) ne démontrent aucun gain de latence : mémoire13,96s contre13,57s, compteurs FS inchangés. Le coût principal, le démarrage, D5 et la qualification runtime restent ouverts ; G09 FAIL et0/12 gates clos.

Verticale en intégration : [profil Docker lié au parent accepté](evidence/2026-11-01/docker-launch-profile/README.md). Douze fonctions uniques passent au gel après correction du défaut STOP sous blob surdimensionné. Clippy a ensuite refusé deux enums trop volumineux ; une indirection privée a été relue, puis Clippy strict, canari de garde, release et trois canaris canoniques V2/V3/V4 passent. Le lot voisin `runtime_prearmed` conserve son résultat historique 3 PASS/1 FAIL avant entrée du callback sous 5 s. Le rejeu diagnostique passe sans attribuer ce timeout ; la revue relève aussi que l'ancien oracle ne prouvait pas la consommation de l'EOF. L'oracle renforcé par barrières causales est importé puis passe 1/1 en 8,80 s dans PRIMARY ; ses budgets de harnais sont distincts des délais produit. Les 18 sources applicatives concordent avec le manifeste final pendant cette intégration, avec formatage et Clippy strict PASS. Aucun ancien résultat n'est transformé en nouveau run. Aucun moteur, retrait D5 ou correctif G09 n'est qualifié par ce lot.

Prochaines barrières de reprise : la revue indépendante du contrat de retrait demande un inventaire durable du gardien et un rapprochement des retraits pending avant le choix Full/STOP du writer. Le contrat est révisé avant implémentation. Le contrôle de supervision schéma3 reste actif seulement dans son origine fraîche : une réouverture le maintient volontairement inactif, sans commande de réactivation sûre livrée. Ce prérequis bloque toujours la recette complète de redémarrage du control plane ; ni une base recréée ni un flag modifié ne la qualifient.

## Exigences d'identité et de configuration — statut au 1er novembre 2026

Le [contrat canonique](../sso.md), issu des décisions du 31 octobre, exige identité locale nominative hors ligne, OIDC public PKCE S256 configurable, mapping administré des permissions, délégations et traçabilité individuelle, découverte Keycloak à quatre comptes et inférence interne sans repli cloud. Les [vues utilisateur et administration à 360°](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) dépendent des permissions et de l'agrégation côté Rust. **Ces parcours restent à implémenter et qualifier** dans le périmètre documenté ici ; les clés `runner`/`viewer`, le dashboard de consultation et les essais API fournisseurs historiques ne les livrent pas.

La gouvernance du checkout principal est intégrée au worktree broker-service en préservant ses preuves datées. Cette harmonisation ne livre aucune commande SSO, stack de découverte ou image d'inférence interne. Les [lots CFG/INF/IDN/SSO/UI/TRC/DEM/DOC](IMPLEMENTATION_PLAN.md#fonctionnalités-demandées--identité-sso-et-découverte-31-octobre), la [matrice de preuves](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) et les [gates](PRODUCTION_READINESS.md) conservent ces écarts ouverts. La [relecture complémentaire du 1er novembre](evidence/2026-11-01/governance/README.md) corrige les mentions contradictoires d'administration UI facultative et de signatures seulement éventuelles dans les documents actifs ; elle ne modifie ni le mandat ni les preuves historiques. Les sections suivantes gardent leurs dates et résultats propres ; l'estimation du 30 octobre n'est pas réévaluée par ces revues documentaires.

## Indicateur de pilotage — 30 octobre 2026

Choix du mainteneur enregistrés dans AGENTS.md, DECISIONS.md et les contrats : Codex/Claude Code en abonnement ou API, dashboard cloud unique par tenant et trois approches Jev (réel, règles Rust imparfaites, petit modèle local). `.env` privé en mode 0600 avec trois clés, ignoré par Git et exclu du contexte Docker/import ; seules les références et les paramètres publics apparaissent dans les exemples. URL Jev confirmée `https://api.typesafe.ai/v1/systemone`, modèle demandé `jev-latest`, paramètres ajoutés sans changer les clés. Aucun lancement automatique ni envoi de projet par ce fichier. L'agrégation cloud demeure obligatoire à implémenter et qualifier.

À la demande du mainteneur : **environ 55–60 % du développement global, incertitude d'environ 10 points**. Estimation d'ingénierie, sans formule ni pondération validée ; ce n'est ni une mesure automatisée ni une promesse de délai. Le socle local identité/API/audit, copie/export, registre/quota/ledger et dashboard est avancé. Le relais métier inter-workers, l'identité worker, Kubernetes, la supervision autonome après crash, la restauration sûre et une partie de l'exploitation restent incomplets ou absents. Cette liste justifie de ne pas assimiler le socle local à un produit terminé. Le nombre de tests ou de lignes de code n'entre pas dans ce pourcentage.

Indicateur de qualification vérifiable séparé : **0/12 gates entièrement clôturés** dans [PRODUCTION_READINESS.md](PRODUCTION_READINESS.md). Chaque lot conserve ses preuves partielles ; un gate ouvert ne signifie pas que tout son code est absent. Réviser l'estimation après livraison des prochains parcours obligatoires, sans l'utiliser comme critère de release. Les tests suivants privilégient Luna, les comportements réels et une sélection selon le risque, conformément à AGENTS.md.

**SSO demandé le31octobre — absent et à implémenter :** OIDC compatible Keycloak, clients publics Web/CLI en Authorization Code + PKCE S256, autorisations et associations tenant côté Rust. Le constat du code est un index de clés runner/viewer (`src/api/auth.rs`) et un token de lecture en mémoire navigateur (`front/src/api.ts`), sans protocole OIDC/PKCE ni intégration Keycloak. [Contrat, configuration cible et qualification](SSO_OIDC.md). Ce SSO du produit est distinct de l'authentification des agents fournisseurs. Aucun service IdP ni compte réel utilisé pour cette mise à jour documentaire.

## Derniers lots — fournisseurs dynamiques et décision locale

**P1 coût durable — PASS logiciel et observations limitées, 1er novembre :** un probe opt-in `cfg(test)` sur vrai Journal/Writer neuf sépare lectures, validations et appels exacts `File::sync_all`, sans changer les corps applicatifs hors tests. RED observabilité manquante puis7 fonctions ciblées PASS, fmt/Clippy strict/release RC0 ; revue indépendante sans P0/P1 identifié.160 sources et binaires hors target vérifiés. Huit mesures debug aux heads0/8 : lectures23–371ms, mutations293/766ms, décomptes de synchronisations attendus confirmés ; temps inclusifs non additionnables. Le graphe H10 plus riche n'est pas reproduit, aucune cause ou correction de ses délais établie. [Preuves, commandes et limites](evidence/2026-11-01/journal-cost/README.md).

L'observation passive du gardien H10 attribué par ses FD donne +4,60s de CPU propre sur environ21s, RSS7664KiB aux deux bornes, sans coût global ou par Arm. L'inventaire initial erroné reste FAIL de harnais ; le filtre FS initial vide est NOT_RUN, corrigé sans rejouer la mesure. FS exact NOT_RECORDED, aucune performance CPU release. La lecture de sources Docker précise les conditions d'un futur témoin de complétion normale, sans autoriser de retrait. Prochaine priorité : graphe plus riche et clôture sûre des slots, sans suppression de barrières ni nouvelle tentative équivalente à l'aveugle. Aucun nouveau moteur, modèle ou déploiement ; G09 FAIL et0/12 gates clos.

**H10 — FAIL d'observation et lacune de visibilité, 31 octobre :** sur le binaire S2,600,04s sans diagnostic public ; le journal conserve pourtant ownership29 puis FailureObserved30 worker/unavailable, avant Created central, Arm ou claim worker. Les deux helpers ont leurs ACK ; aucune durée d'exécution ou cause native du worker n'est établie. L'inventaire37→3 et le nettoyage de cache signalé pendant l'essai empêchent d'en faire une reproduction isolée ; identifiants moteur avant/après égaux, epoch continue non prouvée. Aucune cause attribuée au nettoyage. [Preuves sélectionnées H10](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public).

**Suite S3/S4 et frontend — PASS logiciel ciblé :** deux RED reproduisent la publication manquante et le label non accepté. Le rapport conserve les diagnostics connus ou `failure_observed`, phase connue/effet inconnu, sans texte privé ; contextes ownership/Created ajoutés sans nouvel IO sur succès ni droit/durée modifié.8 fonctions Rust PASS, dont publication réelle Gateway/SQLite/Journal/Writer et API v1 viewer, fmt/Clippy strict/release RC0. Le modèle frontend passe12/12 après trois RED initiaux et un RED complémentaire sur les étapes natives ; build statique RC0 après installation hors ligne, Actions et détail explicites, compteurs/styles/permissions conservés. Revues indépendantes sans P0/P1 restant identifié dans leurs périmètres.159 sources Rust,24 sources frontend et binaires/dist vérifiés indépendamment. Aucun nouvel essai moteur/fournisseur, navigateur/QA visuelle et Node24 de référence NOT_RUN. Le coût du chemin durable et le retrait D5 restent à résoudre avant répétition équivalente ; le [cycle des slots et les pistes Moby](evidence/2026-10-31/broker-runtime/README.md#d5--preuves-encore-nécessaires) n'apportent aucune preuve de fencing. G09 FAIL,0/12 gates clos.

Les entrées suivantes conservent les statuts et prochaines étapes de leur gel historique ; H10 et S3/S4 ci-dessus constituent le point courant.

**H8/H9 — refus préclaim réellement observé, 31 octobre :** H8 s'arrête sur la borne d'observation120s, seulement2,234s après l'ACK Arm ; aucun refus précis n'est alors établi. Le harnais H9 attend dans son budget global et consulte le premier diagnostic via l'API authentifiée. H9 échoue RC101/232,70s avec `readiness_before_claim` / `before_claim` / `slot_phase_mismatch`, événement SQLite seq17 durable et naissance d'outbox locale conservée. Le sample local est absent : l'expiration n'est pas démontrée. Aucun WorkerClaim, Running ou SIGKILL au point prévu ; CID Created et quota conservés. [Preuves H8/H9 et suite S1](evidence/2026-10-31/broker-runtime/README.md#h8h9--observation-et-premier-refus-codé).

**Suite S1/S2 — PASS logiciel ciblé :** réutiliser les données exactes du Draft acquitté et lié à son opération/scope/head retire uniquement la relecture caller après Register. Ce snapshot historique n'est ni le dernier état ni une permission ; mutations suivantes, CAS, préload/postload et durabilité restent contrôlés. Le diagnostic d'erreur expose désormais la phase et la clock validée du slot helper, distincte du sample local.20 fonctions uniques PASS au gel S1 ; Clippy y refuse un idiome d'assertion de test, corrigé seul en S2, sans rejouer les tests ni changer le code applicatif. Fmt/Clippy strict/release S2 RC0 ;159 sources et copies binaires vérifiées. Sur demande du mainteneur, le budget global du harnais devient600s ; l'Arm worker reste5s. Aucun essai moteur sur S1/S2, aucun gain mesuré ou correctif de deadline revendiqué. G09 reste FAIL,0/12 gates fermés ; prochaine étape : un seul essai H10 instrumenté sur le gel final.

**Préclaim et journal lifecycle — PASS logiciel ciblé, 31 octobre :** diagnostics Register/Ready et étapes fermées ajoutés, sans nouvelle autorité ni IO sur succès ; les contextes Exchange ne sont pas une preuve d'envoi/ACK. L'adaptation V3→V4 réutilise le preload strict sous le mutex existant, supprimant un Fullload redondant mais conservant préload/postload, fsync, CAS, floors et bytes d'outbox. Une régression de cette optimisation sur les réservations héritées a été trouvée en revue, reproduite RED puis corrigée GREEN avant intégration.16 fonctions uniques PASS, fmt/Clippy strict/release RC0,158 sources et deux binaires vérifiés ; [preuves et revue](evidence/2026-10-31/broker-runtime/README.md#complément-préclaim-et-adaptation-lifecycle). Aucun P0/P1 restant identifié dans le périmètre relu. Émission du complément sur moteur NOT_RUN, gain de durée non mesuré. Prochaine priorité : une tentative réelle H8 sur ce gel, cinq secondes inchangées, puis correction du refus effectivement observé et qualification panne/reprise/D5. G09 reste FAIL,0/12 gates fermés ; SSO obligatoire toujours absent.

**H7 — FAIL réel préclaim :** après correction de la fenêtre d'observation,122,78s sans Running. Le worker atteint Created rev4 ; WorkerArm ACKrev33, puis FailureObserved34,14/14 opérations ACKées, aucun WorkerClaim ni pending. CID `229b3c948ed9048001362df0acc192414af3a5cb603c038dd553fddfd314316a` reste Created, quota retenu. Le diagnostic D3 ne couvrait pas Register/Ready préclaim, donc aucun événement typé n'a été émis. L'intervalle mtime pendingArm→ACK de4,897s suggère un budget serré mais ne prouve pas l'instant/cause du refus. [Rapport ciblé](evidence/2026-10-31/broker-runtime/h7/core-diagnostic.md). Lot logiciel en validation : compléter cette frontière et supprimer un Fullload redondant avant mutation lifecycle, avec préload/postload et barrières conservés. Aucun gain runtime ni correction de G09 annoncé ; aucun nouvel essai moteur avant validation/revue de ce lot.

**Tentative instrumentée H6 — FAIL d'observation, 31 octobre :** le harnais atteint sa borne préRunning45s alors que la préparation n'est pas terminée. Le journal contient9/9 opérations ACKées, dernière HelperClaim ImportVerification rev22, aucune Arm worker, aucune FailureObserved ni événement `broker.batch.failure`. Le helper de vérification est effectivement terminé exit0/nonOOM avant la publication du FAIL ; le worker reste Preparing rev2. Le retour d'erreur déclenche le Drop des seuls enfants broker/gateway du test. Il ne prouve donc pas un refus de lancement du produit. Racine `/private/tmp/br-6dPLTS` et gardien/ressources conservés. La fenêtre d'observation passe à120s et la borne globale à240s après revue indépendante ; aucun changement applicatif, limite Arm worker5s et oracles de panne inchangés. Une nouvelle expiration d'observation resterait indéterminée. [Preuves H6](evidence/2026-10-31/broker-runtime/README.md#h6--limite-dobservation-atteinte-pendant-la-préparation).

**Verticale Docker du broker — deux FAIL réels H4/H5, 31 octobre :** deux demandes sans fournisseur sur de vrais gateway schéma3 neufs atteignent `StartIssued` avec quota retenu, mais leurs CID restent `Created`. Aucun `Running` observé, donc aucun SIGKILL du broker injecté et aucune preuve de deadline/reprise. Les15 opérations de supervision de chaque demande sont ACKées ; Arm worker ACKrev33, claim `emit_once` ACKrev35, puis `worker/unavailable` rev36/37. H5 remplace le polling du writer par une observation HTTP légère : ce changement ne résout pas l'échec. Le clock du claim laisse respectivement1,984s et2,365s avant les dernières écritures ; la cause précise reste inconnue. Le probe natif ultérieur observe H4 en `stopping`, `prepared_once=true`, CID `created` ; ce bit peut provenir de la restauration et ne prouve pas un Prepare initial réussi. Racines `/private/tmp/br-dFE99I` et `/private/tmp/br-NBcV3F`, gardiens, CID et volumes neufs conservés faute de disarm D5 qualifié ; aucun conteneur/volume du baseline n'a disparu.

**Diagnostic de pré-démarrage — PASS logiciel ciblé :** codes typés aux frontières Prepare/garde/runtime, propagation sans texte arbitraire et événement `broker.batch.failure` raccordé à l'outbox durable existante. `runtime_effect=unknown` ne donne ni terminal, ni fencing, ni libération de quota. Huit fonctions logicielles uniques PASS, fmt/Clippy strict/release PASS au gelD3 ; revue indépendante sans P0/P1 identifié. L'émission du nouveau diagnostic sur moteur est **NOT_RUN**. [Gels, deux échecs réels, commandes et limites](evidence/2026-10-31/broker-runtime/README.md). Prochaine action : une tentative réelle instrumentée sur ce nouveau binaire, puis correction du point effectivement mesuré, sans réarmement, durée allongée ni suppression de barrières durables. G09 reste FAIL,0/12 gates fermés ; SSO/Keycloak/PKCE obligatoire toujours ABSENT/NOT_RUN.

**Acteur STOP restreint — PASS logiciel ciblé, 31 octobre :** premier Cancel sans blob et ouverture à froid partielle reproduits puis corrigés. Une seule file/lease, aucune conversion STOP vers le métier, nouvelle annulation refusée si budget inconnu et reçu conservé au rejeu. Résultat Unknown en queue résolu sans doublon ; les lecteurs métier gardent leurs contrôles avant/après publication. Trois écarts de revue corrigés : double acteur, barrières Full et perte du hash de rejet au transfert. Le diagnostic réel `broker recovery-status` distingue journal confirmé et moteur non interrogé. 33 fonctions uniques sur les gels reliés, fmt/Clippy strict/release PASS ; [preuves, échecs de fixtures et limites](evidence/2026-10-31/stop-only-actor/README.md). Vrai gateway/Store A/B et connexion Unix exercés, sans worker/moteur ; le canari de pile ne couvre que le refus vide. G09 reste FAIL,0/12 gates fermés. Prochaine priorité : verticale Docker réelle du broker et cycle du gardien/D5. SSO/Keycloak/PKCE reste ABSENT/NOT_RUN, exigence obligatoire déjà documentée.

**Rejeu d'annulation durable — PASS logiciel ciblé, 31 octobre :** un replay retente les seules Arms nées avant le Cancel exact, avec reçu historique, sans révoquer les exports postérieurs. Projection metadata-only du writer vivant, sans lecture du blob fournisseur, lease et planchers conservés ; port STOP borné avec intention retenue à travers les remplacements. Le P1 de sélection du cache de remplacement découvert en revue est corrigé. 14 fonctions uniques ciblées, fmt/Clippy strict/release PASS ; [gels, FAIL de harnais et limites](evidence/2026-10-31/cancel-retry/README.md). Natif réellement lancé mais actors NotReady, aucun moteur. Premier Cancel sans blob et writer strict indisponible restent obligatoires à traiter ensuite ; la projection seule ne les ferme pas. G09 reste FAIL,0/12 gates fermés ; prochaine priorité : porte STOP-only indépendante du writer métier, puis verticale réelle du broker.

**Publication d'export — PASS logiciel ciblé, 31 octobre :** durabilité conditionnée aux synchronisations des fichiers/modes, répertoires et parent ; capacités d'ascendance conservées et politique UID/modes/ACL appliquée aux exports. Une erreur après renommage conserve la destination avec résultat inconnu. Scanner/export bornent les parents implicites et la profondeur64. Les défauts ont été reproduits avant correction ; 5 tests export, 11 capacités source/export et 1 vrai bootstrap public du gardien passent, ainsi que fmt/Clippy strict/build release. Revues indépendantes traitées, [preuves et limites](evidence/2026-10-31/export-publication/README.md). Le bootstrap ne crée aucun worker. Erreurs matérielles, panne électrique, matrice multi-UID/plateformes, service complet et deadline réelle restent non qualifiés ; G09 demeure FAIL, 0/12 gates fermés. Prochain P1 : convergence des annulations après stop RPC perdu, puis verticale broker réelle. La borne de l'énumération totale des sources reste ouverte.

**Broker permanent — intégration en cours, 31 octobre :** service Rust, profils de confiance, IPC privé, CLI explicite, writer borné, outbox V4, capacités de copie/export et gardien séparé sont raccordés dans le worktree `broker-service`. Fmt, Clippy strict all-targets et build release passent. Les 40 fonctions ciblées passent dans leurs gels documentés : IPC/CLI/writer5, ownership2, service10, gardien1, workspace4, outbox5, checkpoint6, bindings2 et cinq parents de faults/locks du journal. Les deux helpers ignorés et les replays ne sont pas additionnés. Le test du gardien observe SIGSTOP/SIGCONT, refus des admissions pendant le gel, même epoch puis arrêt propre ; il ne crée aucun worker et ne prouve pas une deadline Docker. Échecs de harnais et correction du manifeste auto-référent conservés ; 149 véritables fichiers source vérifiés. [Preuves et limites](evidence/2026-10-31/broker-service/README.md). Aucune validation globale du service n'est annoncée.

La revue a conduit à séparer la récupération STOP du chargement métier : un sibling endommagé ou un credential disparu ne doit pas effacer les arrêts prouvés. Elle a également corrigé en source l'application d'un ancien Cancel aux exports postérieurs, la vérification de santé avant admission et les collisions entre identités de commandes. L'inventaire partiel ferme les admissions et les anciennes histoires managed sans Arm restent inconnues. Le reader normal conserve ses refus stricts. Ces corrections attendent encore leurs preuves complètes et les tests sur moteur. Le nouveau service ne remplace pas le CLI par défaut ; gardien TTY, fencing/disarm D5, reprise métier et qualification Docker/Podman restent ouverts. **G09 demeure FAIL et 0/12 gates sont fermés.** Le rappel Daybreak Blue attend les scénarios réels d'arrêt/reprise de workers, pas le seul test du processus gardien.

**Cycle de vie local V3 et admission HTTP — PASS logiciel ciblé, 31 octobre :** rejet avant intention, commandes locales d'approbation/cancel/erreur, lecteurs status/list et reprise dans la même chaîne du Journal, sans changer les octets/domaines V1/V2. Le client conserve les refus HTTP403/429 typés et contrôle le Worker initial. BLC-01 à04 reproduits puis corrigés ; revue logicielle close. Les 35 fonctions uniques ciblées incluent contrat API, diagnostics et self-test, sans additionner les deux helpers enfants ; fmt/Clippy all-targets strict/release PASS. [Preuves et limites](evidence/2026-10-31/broker-lifecycle/README.md). CancelRequested ne libère rien : CancelReserved libère la quota dans la transaction centrale du Store, avant ACK local ; une réponse perdue peut laisser un pending local avec quota centrale déjà libérée. Preparing sans CID et cible jamais démarrée sans terminal admissible peuvent conserver quota/NeedsReconciliation. Service/IPC/enforcer, gardien, D5 et cutover CLI restent ouverts ; aucun nouvel essai frontend/moteur/modèle/E2E/cloud. QA, schéma2 et `billable=false` inchangés ; G09 reste FAIL, 0/12 gates fermés.

**Préparation/reconstruction et transport préarmé — PASS logiciel ciblé,31octobre :** préparation avec UUID imposé et FD figés, reconstruction pure avec approbation conservée, handoff exclusivement issu du binding durable même sans anciennes sources/configurations fournisseur. Nouveau port batch sans timer métier ni Ctrl-C global, dernier garde consommé avant spawn, signal d'arrêt reçu pendant les callbacks bloqués. RPT-01 reproduit puis corrigé : conserver le même futur après EOF évite une seconde invocation/RPC. Neuf fonctions nouvelles et15canaris sélectionnés PASS, fmt/Clippy strict/release PASS, revue logiciel CLOSE ; [preuves et limites](evidence/2026-10-31/broker-submission-prearmed/README.md). Manifeste118entrées et artefacts natifs hors target, binaire `c80bf668f142ca0184ef947a77d7c3344ebef7e3cefd33e510d9dd9558129278`. Journal de fichiers réel, CLI runtime Rust synthétiques, aucun Store SQLite/moteur/modèle pour les nouvelles cibles. Ces ports ne sont pas encore l'enforcer ou le service permanent ; raccord du cycle de vie local au service, IPC/batch/helpers/gardien/D5 restent à livrer. CLI par défaut, QA, schéma2 et facturation inchangés ; G09 reste FAIL,0/12gates fermés. Rappel Daybreak Blue demandé après stabilisation de ce service et de ses tests réels d'arrêt/reprise.

**Checkpoint de supervision V2 — PASS logiciel ciblé GEL5 :** révisions typées avec vrais DTO du contrôle et publication/CAS communs, plans/rev0 V1 inchangés, historique managed et downgrade refusés, requêtes pending et réponses/snapshots immuables. Résultats dérivés du reducer Store pur réutilisé ; admission préparatoire pending/refusée conservée sans Worker/effet avant ACK. BCP-01 à06 traités, dont quatre RED de données invalides acceptées, branches ACK contradictoires et rejet d'une vraie préparation. Six fonctions PASS release et26canaris pertinents, fmt/Clippy/release PASS ; faits runtime/horloge synthétiques, Store/journal réels. [Preuves et limites](evidence/2026-10-30/broker-checkpoint-v2/README.md). Source/test115entrées et binaire natif `b360af55dc6d6b75dd9d9f9ead2d59eecda0428cbe7f68b5cfa833b0a0911f98` préservés. Coût du graphe17,59s release sur cet hôte, pas une capacité100 ; anciens benchmarks ont une limite d'archive du test, explicitée. QA, schéma2 par défaut et facturation inchangés. IPC/ordonnanceur, gardien et D5 restent à livrer ; G09 reste FAIL et aucun gate global fermé.

**Bootstrap neuf du contrôle3 — PASS ciblé natif et Docker :** constructeur exclusif, backup2 vérifié, ancêtres synchronisés, migration3 active dans la seule origine fraîche ; reopen reste en maintenance et défaut2 inchangé. Option gateway `--initialize-supervised-stores BACKUP_ROOT`, registres A/B et séparation physique des dirs credentials/backups. Sept fonctions bootstrap et39canaris distincts PASS, fmt/Clippy/release ; fixture de clone rendue explicitement0600 après reproduction de sa dépendance à umask022 sur baseline. [Preuves et limites](evidence/2026-10-30/supervision-bootstrap/README.md). Docker Desktop/LinuxARM64 : témoin root0755 refusé avant DB, image avec `/run/secrets`1000/0700 acceptée, vrai HTTP A/B actif3/namespaces distincts/backups privés, rootfsRO/réseau none/caps retirées ; aucun worker. Binaire natif `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39`, Linux `764b91d0297669c0c07db094dd46fb4874f5145bb8966f330a62138b9cc94e9e`, préservés. Images de test par overlay, Dockerfile multi-stage complet non reconstruit ; QA/facturation inchangées. IPC/ordonnanceur/gardien et réactivation/fencing toujours ouverts, G09 reste FAIL.

**Complément contexte managed/plancher — PASS ciblé, correctif livré au composant :** la cartographie du runner a révélé que le handle worker porte l'opération du plan, et non l'UUID de tentative centrale attendu par le nouveau validateur et son ancienne fixture. RED reproduit après correction de la fixture, puis comparaison alignée sur le producteur sans changer les starts/plans/claims. Douze tests du journal PASS, dont six refus de contexte, parcours initial→managed→reopen et refus de rollback par writer/clones avec ancien graphe valide relisible après reopen. Store réel, autorité/observations synthétiques ; aucun moteur. Fmt/Clippy/release et revue source PASS ; [preuves et limites](evidence/2026-10-30/journal-context/README.md). Nouveau binaire `b884e84305f125cdaae610881ecc4e747f3ede0a5e634e4cf3391b5a01482ec6`, hors dépôt, QA inchangée. Les NOT_RUN managed/plancher cités au gel initial ci-dessous sont ainsi couverts au périmètre batch/generic/None ; ni anti-restore après restart ni supervision autonome qualifiés.

**Journal privé du broker — PASS ciblé logiciel/filesystem :** persistance scope/Submit/intention/checkpoint/bindings None, manuel et fournisseur ; fichiers exclusifs synchronisés, verrou retenu, rejeu sans nouvelle source, révisions/CAS et refus conservateurs. Sur macOS :10 intégrations,4 fonctions unitaires dont un self-test de harnais et deux helpers explicitement lancés,1 preuve SQLite même PID,26 canaris historiques ; fmt/Clippy strict et release PASS. Les mêmes15 fonctions, sans les26 canaris, passent sous Linux ARM64 dans la VM Docker Desktop, source111/111 recopiée conforme ; aucun worker ou modèle. Défauts nullable, versions imbriquées et course du verrou corrigés. [Preuves, limites et rétention des RED](evidence/2026-10-30/broker-journal/README.md). Binaire `39d88ea07a6ec7de50288d726ce263647cbde20bd7fbfd3cce28483a8efd22a4`, conservé hors dépôt ; aucune image de contrôle ni QA remplacée. La contextualisation managed complète via Journal et le rollback contre son plancher mémoire restent sans test ciblé avant activation. IPC/service/ordonnanceur/gardien et D5 restent à livrer ; G09 demeure FAIL et les12gates restent ouverts.

**Intention immuable — PASS logiciel ciblé :** projection des champs initiaux du vrai RunPlan, séparée du checkpoint managed ; nouveau format strict, hash de domaine propre et choix fournisseur figé cohérent. Aucun accès aux sources, au réseau ou au TTY. Écart de revue BIR-01 sur diagnostics des snapshots publics reproduit puis fermé par wrappers opaques, sans changement du golden. Six tests finaux et douze canaris historiques ciblés PASS (canaris non répétés après le seul delta d'opacité), fmt/Clippy/release et revue acceptés. [Preuves et limites](evidence/2026-10-30/broker-intent/README.md). Binaire de ce lot `831a3e19854e29f35dade316f71cb463c43faacf49f5f96204277b5ac85b8e1a`, préservé dans le dossier privé de preuve ; aucune image de contrôle ni QA mise à jour. Ce format seul ne livre aucun ACK ou binding durable ; le composant journal ci-dessus a depuis ses preuves distinctes. IPC, service/ordonnanceur/gardien et D5 restent à livrer ; une identité d'invocation n'est pas une autorisation tenant.

**API de supervision et horloge native — PASS ciblé :** neuf routes HTTP v1, identité/rôle avant corps, helpers liés à leur parent durable et refus de nouvelle émission en maintenance. Quatre tests API Rust/Axum/SQLite, cinq tests d'horloge dont deux processus macOS et vingt régressions API passent ; le helper enfant ignoré n'est pas un PASS supplémentaire. Le client Jev réellement configuré n'ouvre aucune connexion pour le rejeu exact et le refus frais inactif testés, après contrôle positif d'une connexion locale. Revue indépendante, fmt, Clippy strict et release PASS. [Preuves et limites](evidence/2026-10-30/supervision-api-clock/README.md). Binaire `d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31`, sans image de contrôle ni déploiement QA. Hôte Linux natif/x86_64, sommeil/reprise, reboot, service permanent, IPC, ordonnanceur, gardien et D5 restent ouverts ; ce lot ne corrige pas encore le FAIL du crash CLI.

**Complément horloge Linux ARM64 — PASS ciblé :** les mêmes cinq fonctions, dont lectures natives et second processus, passent sur le noyau Linux7.0.14 de la VM Docker Desktop. Source du gel11eb copiée, compilation offline/réseau none, UID1000 et ressources bornées, sans montage/secret/socket. Recopie depuis le conteneur arrêté conforme aux sources/Git et revue de preuve acceptée. [Preuves](evidence/2026-10-30/broker-clock-linux/README.md). Aucun worker ou service broker qualifié, aucune attribution au code d'intention ultérieur ni nouvelle preuve de sommeil/reprise ou d'enforcement.

**Store de supervision — PASS logiciel ciblé :** transactions/claims worker et helpers, migration sauvegardée et replay strict du schéma3 ; origine neuve privée et watermark mémoire partagé protègent copies/rollback testés. Mismatch d'import persistant et helpers pending bloquent la suite ; ancien Created reste maintenance sans nouvelle durée. 94 tests uniques, puis24 concernés rejoués après mutex final, revue close, fmt/Clippy/release PASS. [Preuves et limites](evidence/2026-10-30/supervision-store/README.md). Artefact de ce gel `8168c30213966093dd907a1c9b290968cda45dca4a3be908341bd911998f59d0`, préservé puis remplacé par le lot API/horloge ci-dessus ; défaut2 inchangé, aucune route, image ou QA ajoutée par le seul lot Store. Réactivation après restart, D5, broker permanent et gardien restent obligatoires et non livrés.

**Arrêt sur snapshot — PASS logiciel ciblé :** refus avant kill si CID/image/configuration/ressources diffèrent ou si l'inspection est inconnue ; six raccordements runner et branches timeout/annulation possédant une baseline durcis. RED conservé, 32 tests ciblés PASS, revue indépendante acceptée, fmt/Clippy/release PASS. [Preuves et limites](evidence/2026-10-30/runtime-stop-snapshot/README.md). Artefact de ce gel `ac13f27c613c557e86dd27e48eaa2468deb7b1f048e91e61a7be40819a69e753`, préservé puis remplacé par le lot Store ci-dessus, sans redéploiement QA. Les [DTO/helper policy préparatoires](evidence/2026-10-30/supervision-contract/README.md) et leur canari legacy gardent leurs preuves propres ; aucun résultat moteur transféré.

**Crash du CLI — FAIL runtime reproduit :** sur Docker Desktop/macOS ARM64, worker `sleep20` avec limite5s encore Running7,0019s après son observation initiale et SIGKILL du seul CLI (signal9 confirmé). Deux essais distincts : défaut de comparaison du harnais au premier, puis unique retry corrigé ; arrêt/réconciliation ciblés, audit18 valide/quota0/une mesure non facturable et volume conservé pour chacun. [Preuves](evidence/2026-10-30/cli-crash/README.md). Binaire testé `91f0cc10…2ae5a9` préservé ; le nouveau broker permanent/gardien reste à implémenter. Le port strict d'arrêt en cours ne corrige pas cette perte d'échéance.

**Arrêt HTTP/SSE — PASS ciblé :** gateway/dashboard drainent cinq secondes après signal, comme l'ingress existant ; signaux Unix installés de manière fallible avant readiness. Vrais flux maintenus plus de six secondes avant SIGTERM, puis sorties observées en 5007/5010 ms ; repos SIGINT/SIGTERM en 1 ms. Deux tests ciblés et trois régressions ingress/tenants PASS, revue close, fmt/Clippy/build PASS. [Preuves](evidence/2026-10-30/http-shutdown/README.md). Binaire de ce gel `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9`, préservé puis remplacé par l'artefact ci-dessus ; pas de borne absolue sous IO/OS bloqué, pas de qualification des workers après mort du broker ni de redéploiement QA par ce lot.

**Direction broker approuvée par le mainteneur :** [service permanent, IPC privé et gardien TTY](BROKER_SUPERVISION.md), sans installation de service OS ni action cloud/PKI autorisée par cet accord. Revue de conception BSR-01/02/03 traitée (intention/checkpoint distincts, restauration indépendante de Drop, stop indépendant des fsync). Contrats, Store, API, producteur d'horloge, format d'intention et composant journal ont leurs preuves séparées ; raccord au service/IPC, ordonnanceur et gardien restent à livrer. Fencing du start ambigu et récupération macOS figé restent à développer et vérifier. Ne pas confondre composants préparatoires et qualification autonome.

**API authentifiée avant extraction — PASS ciblé logiciel et HTTP local :** index des credentials construit au démarrage, guards typés avant Path/Query/Json, refus des Bearer ambigus ou de plus de 4096 octets, viewer refusé avant lecture d'un corps de mutation. Les configurations partagées sont en Arc et les tokens ne sont plus clonés dans chaque TenantState. RED 4 groupes en échec/2 déjà sûrs, puis 6 nouveaux tests et 45 régressions PASS, fmt/Clippy strict et revue indépendante acceptés. Le vrai TCP reçoit son premier statut 401 sans envoyer le corps annoncé. [Preuves et limites](evidence/2026-10-30/api-auth/README.md) : ni drainage/fermeture bornée, ni charge/DoS général qualifiés par ce seul lot. Binaire de ce gel `b7565088f8e10c05d24b5cc3b28c33bc993760b35cc3479322af9d63d72ec191` ; le lot HTTP suivant ci-dessus fournit un nouvel artefact. Pile QA encore ancienne, aucun nouveau résultat fournisseur/moteur transféré.

**Implémenté et vérifié au périmètre logiciel :** configuration versionnée, `providers init/status`, résolution Codex/Claude Code et API/abonnement, bootstrap Rust, isolation des réglages/environnements, transmission du credential minimal, contexte lié au hash de décision/admission et plan privé v3 stable pour la reprise. Les contrats historiques sans contexte gardent leurs hashes ; contexte explicitement nul/incohérent refusé. Pas de fallback silencieux vers une API facturée. [Preuves](evidence/2026-10-30/provider-auth/README.md) : suite complète avant revue 388 PASS/0 FAIL/8 ignorés ; corrections PRV-01/02/03 relues, puis 25 PASS/0 FAIL/1 fixture ignorée, fmt/Clippy strict/release PASS. Ces compteurs se recouvrent et ne s'additionnent pas.

**Delta de sûreté Jev précédent :** les noms `JEV_API_KEY` et `JEV_API_KEY_FILE` sont refusés dans le handoff manuel avant snapshot d'un nouveau run, pour garder la clé du contrôle hors worker. Une régression paramétrée RED/GREEN est rapportée dans l'historique outil ; pas de log brut archivé. [Revue indépendante source](evidence/2026-10-30/provider-auth/jev-handoff-review.md) sans nouveau défaut confirmé. Après les essais natifs sur l'ancien binaire préservé, release reconstruite avec ce delta : fmt/Clippy/build PASS, sources89 inchangées, sans nouvelle suite de tests. Binaire de ce gel `ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917` ; ne pas lui transférer les essais natifs du binaire précédent. Le lot API ci-dessus fournit depuis un nouvel artefact.

**Image construite et probes hors réseau PASS :** [worker géré](evidence/2026-10-30/provider-auth/worker-image.md), Linux ARM64 sur Docker Desktop, Codex0.165.0/Claude2.1.140 réels et refus bootstrap sans handoff vérifié. Pas de credential ni modèle dans ces probes. npm a refusé le postinstall Claude ; le batch géré ultérieur ci-dessous a néanmoins fonctionné. Les autres chemins gérés, authentifications abonnement, TUI clients à deux tours, renouvellement de tokens et réglages adversariaux restent non qualifiés. `providers status` décrit une configuration, pas la disponibilité réelle ; probes clients après admission dans le worker. L'ancienne pile QA ne reçoit pas ces nouveaux contrats.

**Claude API batch manuel PASS ciblé :** un vrai appel via Claude Code 2.1.140, deux fichiers créés/exportés exacts, original intact, terminal/cleanup/quota libéré, une allocation observée `billable=false`, audit 26 valide. Ancien alias `haiku`, sans attestation du modèle servi 5.5. **Claude TUI FAIL par instrumentation** (handle perdu), sans second message ni preuve d'une réponse modèle ; arrêt ciblé, audit 32 valide/quota libéré et gateways privés arrêtés. Les [rapports et résultats](evidence/2026-10-30/provider-cli/README.md) ne deviennent pas une qualification des nouveaux profils gérés. Codex batch manuel reste PASS et Codex TUI deux tours reste non qualifié.

**Claude API batch géré ensuite PASS ciblé :** une invocation via `providers init/status` et `run --client claude-code --auth-mode api`, sans argv fournisseur manuel. Modèle demandé explicitement `claude-haiku-5-5` (modèle servi inconnu), fichier42octets créé/exporté exact, original intact, exit0/cleanup/quota libéré, une allocation observée non facturable, audit36 valide après redémarrage gateway. Deux préflights échoués sans worker/modèle (gateway mort puis exception Docker Desktop manquante) conservés, sans retry fournisseur. [Rapport](evidence/2026-10-30/provider-auth/managed-claude/real-run.md). Délai75s, pas de borne monétaire du preset. Les autres modes, TUI et paramètres adverses restent ouverts. Le warning postinstall Claude n'a pas empêché ce batch.

**Modèle local réel : mémoire/protocole PASS, sémantique FAIL.** `Qwen2.5-0.5B-Instruct Q4_K_M`, contexte 512, deux appels Docker Desktop ARM64, pic cgroup 777 912 320 octets sous plafond effectif 999 997 440, sans OOM et sans swap. Il répond `allow` à une suppression synthétique : candidat refusé pour autoriser des actions. [Preuves et entrées/sorties](evidence/2026-10-30/local-judge/README.md). Ni budget 500 Mo, ni Podman, ni egress actif ne sont qualifiés. Le sélecteur commun des trois moteurs et l'adaptateur local produit restent absents ; le moteur Rust n'est pas une compréhension sémantique exhaustive.

**Réutilisation du backend,1er novembre : PASS ciblé.** Après réacquisition des images absentes, quatre cas de transport ont reproduit/corrigé la lecture fragmentée du probe, puis une seule salutation réelle Chat Completions a réussi. Pic788877312 octets, sans OOM ; serveur arrêté ensuite. [Sources, sorties et limites](evidence/2026-11-01/local-inference/README.md). Le FAIL sémantique historique reste ouvert. Ce n'est ni un service produit permanent, ni un raccord Codex/Claude, ni une qualification entreprise/TLS ; aucun gate clos.

**Jev réel PASS ciblé :** un seul POST externe via le client Rust du gateway, demande `jev-latest`, réponse `jev-1.14.0`, `allow`/confiance1,0 sur un état synthétique public. Décision locale conservée, chaîne d'audit réouverte valide (3 événements) et base inchangée. Aucun worker, projet, retry ou appel catalogue. [Preuves](evidence/2026-10-30/jev-real/README.md). Ce smoke n'est pas une qualification générale des décisions ni de la disponibilité du service.

## Candidat QA courant — journal moteur désactivé, 30 octobre 2026

Le [lot preset batchv2](evidence/2026-10-30/engine-logs-v2/README.md) est implémenté et relu : nouvelles créations `--log-driver none`, inspection avant claim/start, fingerprints privés versionnés et lecture/arrêt des anciens workers v1 conservés. Premier start historique v1 refusé sans réarmer les claims. Régressions ciblées, fmt/Clippy/release et essai Docker réel de capture masquée/ACK503 PASS ; pas de suite globale ni de matrice E2E relancée par réflexe. Le driver antérieur observé était `local` avec rotation native, sans fuite confirmée par cette baseline.

Les trois services utilisent `agentsandbox-control:qa-20261030-engine-logs-v2-final-c53bd8`, ID `33313bfcc385be5056e722cafd891f943e5cdb2ddf3849035a7767d2f1dcf543`, Compose `/tmp/ai-guard-registry-qa.40IImn/compose-engine-logs-v2.yaml`. Audit668 identique avant/après mise à jour sauvegardée,19workers v1 lisibles. Un nouveau `/usr/bin/id` v2 réel confirme UID1000, exit0, terminal/cleanup/quota libéré et une allocation observée `billable=false` ; audit690 valide. Dashboard partagé sans montage, UI OVPT conservée. Données, credentials, images et preuves précédentes préservés.

Le [Codex0.165.0 batch réel](evidence/2026-10-30/provider-cli/README.md), sous gelSQLite327, a créé/exporté le fichier exact sans changer l'original après un premier FAIL bwrap conservé. Modèle demandé Luna, sans attestation indépendante du modèle servi. Le gel initial Claude2.1.140/Codex versions/aides est ensuite complété par l'essai Claude batch ci-dessus. **Clients TUI non qualifiés :** premier essai Codex, effet/export du premier tour réels mais second texte resté dans le composeur, arrêt contrôlé137, audit33/quota0/allocation1. Diagnostic sans clé/réseau/modèle : `/status` et `/exit` fonctionnent avec texte, pause500ms puis CR séparé. Nouvelle session ensuite : premier fichier réel, aucune preuve visible de reçu/idle, second message non envoyé ; timeout300s puis137/timed_outtrue, audit26/quota0/allocation1 et export exact une ligne. **Deux échecs du critère deux tours**, sans défaut broker établi : une réponse d'outil tronquée a pu contenir le reçu, contenu non vérifiable. Le diagnostic ultérieur hors ligne a observé deux outils locaux ; `/exit` restait non qualifié. [Rapports distincts et limites](evidence/2026-10-30/provider-cli/README.md).

Le [quick start](../quickstart.md) et l'entrée README sont rédigés, vérifiés contre le CLI/code et relus : commandes, prérequis, configuration, refus, copie/export et limites. [Preuves](evidence/2026-10-30/quickstart/README.md). Le mainteneur a reporté le nouvel audit global Daybreak après cette nuit de travail ; aucun lancement ni planification automatique. Les revues ciblées d'ingénierie continuent.

**CLI TTY générique implémenté et vérifié, non déployé dans la pile QA :** mode lié aux demandes/approbations, preset interactif versionné, filtre streaming et restauration. Contrats/audit/registre/reprise ciblés PASS ; les défauts de revue marges/drain et l'alias `/dev/tty` Darwin sont corrigés, avec RED/GREEN conservés. Dernier gel : terminal12/12, autorisation8/8, fmt/Clippy alltargets/release PASS. Docker Desktop/macOS ARM64 réel : saisie/TTY/resize/exit7 en12,89s et arrêt doubleCtrlX en13,90s PASS. Audit réouvert valide, allocation observée non facturable unique, quota libéré et original intact dans les deux cas. Pas de témoin concurrent pour l'arrêt. [Preuves, deux FAIL préalables et limites](evidence/2026-10-30/interactive-cli/README.md).

Revue finale : l'ancien test CtrlX ne distinguait pas explicitement le motif opérateur d'un timeout. Le résultat13,90s reste daté avec cette limite. Test renforcé puis unique rejeu PASS13,57s : motif CtrlX exact dans l'audit rouvert, `timed_out=false`, réponse après seconde frappe <30s et allocation unique. L'écart d'assertions est relu et fermé. Aucun témoin concurrent. La boucle du harnais sous sortie continue est ensuite corrigée avec une régression Rust locale RED/GREEN (pompe bornée à 4 KiB, service du contrôle entre lectures), sans nouveau moteur/fournisseur. La réserve distincte des ACK HTTP attendus dans le runtime reste ouverte : aucune borne murale stricte de trois secondes n'est promise.

Source générique gelée `/tmp/ai-guard-interactive-qualified.hO9L2x/source.tar`, SHA256 `348a6c2aeeb7a015114e2c47f8794e41a67b512ccbac27b86d4402d6dfe33207` ; binaire macOS `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`. Une troisième sonde Docker fraîche PASS12,29s vérifie les octets exacts CR/LF/Ctrl-C/UTF-8/flèche dans un terminal distant raw, la restauration avant ACK, l'audit et le quota ; log Cargo original conservé. Seuls test/helper/image probe ont évolué après ce gel, sans changement production. Les vrais clients Codex/Claude TUI ne sont pas qualifiés par ces sondes. Méthode d'authentification Claude demandée au mainteneur sans copie du cache hôte. Réattachement après claim consommé et durée autonome restent ouverts. Le candidat QA actif reste batchv2 décrit ci-dessus ; aucun résultat TTY attribué à cette ancienne image.

## Lot sécurité précédent — SQLite privé, 30 octobre 2026

StoreStorage ancre les parents privés, impose main/sidecars600 et nom ASCII, sérialise l'initialisation et laisse SQLite posséder ses FD. Une régression POSIX a reproduit puis corrigé la perte de locks au drop de l'ancien Store. Migration/backup consomment le même garde.327tests top-level/0FAIL/3ignorés, fmt, Clippy alltargets strict et release PASS ; deux essais Docker ignorés exécutés séparément PASS. Linux dev :41intégrations+1VFS+1ACL et sonde réelle à deux UID PASS, revue indépendante sans bloquant. Les enfants F_GETLK ne sont pas recomptés.

QA désormais sur `qa-20261030-sqlite-327-ovpt-8d29af`, ID `32cdcf1f3719015701966ab6b80da2104281ff1c42226cced79e0f541b9f034b`, trois services sains et UI OVPT conservée. Avant mise à jour :17workers terminaux et0quota retenu. Après copie à froid privée et changement explicite offline du seul répertoire/data755→700, hashes des deux bases inchangés ; nouveau Store retrouve591événements et même tête. Aucun schéma, credential ou claim modifié automatiquement. [Preuves, compatibilité ASCII, FAIL initiaux et limites](evidence/2026-10-30/sqlite-storage/README.md). Restauration sûre des claims, pannes et qualification globale restent ouvertes.

## Dernier lot visuel vérifié — OVPT et sombre, 30 octobre 2026

La signature OVPT, ai-guard comme nom de produit, le favicon vectoriel OV à16/32px et le thème sombre en noirs neutres sont livrés dans le dashboard QA. Le bouton désactivé devenu olive par opacité a été corrigé après revue. Les 27 tests Node24, build, parcours navigateur, revue indépendante puis dix contrôles sur Rust/HTTP réels passent. Les captures desktop/mobile et les limites sont dans le [dossier OVPT](evidence/2026-10-30/ovpt-ui/README.md).

Lors du déploiement visuel initial, seul le dashboard avait changé d'image (`qa-20261030-ovpt-ui-final-5b92c7`, ID `c841cca6a5e8789a2d0fcf41932193ae1e130e214a4e08ed171ba847b314564a`), en conservant le binaire Rust gel302 ; gateway/ingress restaient sur `qa-20261030-editorial-fs-6f4d8b02`. L'audit589 était identique avant/après ; les deux événements déclarés du harnais l'ont amené à591, chaîne valide. Ce résultat historique reste distinct du lot SQLite déployé ensuite décrit ci-dessus.

## Inventaire initial

| Élément | Constat initial | Preuve |
|---|---|---|
| Domaine et API | Rust, axum/tokio, rusqlite SQLite WAL, reqwest/rustls ; gateway, dashboard et commandes opérateur dans un paquet | `Cargo.toml`, `src/lib.rs`, `src/main.rs` |
| Audit/politique | Présents ; masquage, chaîne de hash, limites, allowlist et décisions | `src/audit.rs`, `src/policy.rs`, `tests/` ; nouvelle validation en cours |
| Runtime | Docker/Podman présents ; copie/export filtrés, contrôles d'arguments | `src/runtime.rs`, `src/workspace.rs`, `src/runner.rs` ; confinement réel non qualifié |
| Front | TypeScript sans framework, Vite, API/SSE et tests Node/navigateur | `front/` ; baseline en cours |
| Identités multi-client | **Absent** : une `Store`, un runner et un viewer globaux au démarrage | `GatewayState` et `GatewayArgs` initiaux |
| Registre, quotas de workers, grants, relais et ledger | **Absents** du code initial, malgré des affirmations contraires dans le document racine de préparation | Pas de `src/tenancy.rs` ni `src/control.rs`, routes dans `src/api.rs` |
| Kubernetes | **Absent** | `RuntimeKind` ne contient que Docker/Podman ; aucun déploiement Kubernetes |
| Documentation de travail attendue | `docs/engineering/`, `PROMPT_PRODUCTION.md`, `CLAUDE.md`, `.claude/rules/`, `.agents/skills/`, `.claude/skills/` **absents** initialement | Inventaire `rg --files --hidden` et contrôles de présence |
| Skills projet | Chargement automatique non confirmé ; les quatre `SKILL.md` projet sont absents | Ne pas prétendre les avoir exécutés ; procédures explicites utilisées |
| Release open source | Licence et canal privé de vulnérabilités absents, décisions du mainteneur nécessaires | Inventaire racine |

## Cinq écarts prioritaires initiaux

1. **P0 — Isolation client absente** : jetons et base globaux ; aucun contrat authentifié A/B livré. Implémenter la liaison identité → tenant → base avant toute revendication multi-client.
2. **P1 — Autorité durable de cycle de vie absente** : pas de registre de workers, réservation atomique ni réconciliation d'usage. Le runner local ne fournit pas ce contrôle multi-client.
3. **P1 — Qualification du confinement manquante** : les tests d'arguments/fixtures et le smoke ignoré ne prouvent pas les effets Docker/Podman. Les matrices restent à exécuter séparément.
4. **P1 — Reprise et évolution du stockage incomplètes** : version de schéma, sauvegarde/restauration et ledger doivent être prouvés avec pannes et reprises ; le hash d'audit seul est insuffisant.
5. **P1 — Cloud non implémenté** : adaptateur Kubernetes, identité workload, isolation réseau appliquée et capacité 100 workers restent à développer puis qualifier.

La qualification production globale reste **non acquise**. Les résultats courants sont consignés dans [VALIDATION.md](VALIDATION.md) et le chantier actif dans [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).

## Avancement du premier cycle

- Implémenté et vérifié au niveau logiciel : sélection identité → tenant → SQLite, liaison durable, refus des credentials ambigus, isolation des routes d'audit/décision/SSE/export, alias `/api/v1`, registre CLI de références de fichiers et redémarrage d'un vrai gateway HTTP avec conservation des données A/B.
- Suites intégrées successives : **109 PASS/0 FAIL/1 ignoré**, puis **121 PASS/0 FAIL/2 ignorés**, puis **139 PASS/0 FAIL/2 ignorés** après les correctifs finaux ingress et terminal runtime. Les deux tests réels restent ignorés dans cette commande globale et ont leurs exécutions explicites séparées. Fmt, Clippy et build release PASS sur Rust 1.99.1/macOS ARM64 ; les [preuves conservées](VALIDATION.md) identifient chaque candidat et sa portée.
- La revue indépendante a reproduit puis revalidé la fermeture de trois P0 : trigger SQLite supprimant un événement pourtant acquitté ; corruption externe non détectée avant une nouvelle décision ; hardlinks entre deux processus créant des WAL divergents et une perte d'audit. Schéma strict/versionné, revalidation avant append et refus des alias physiques sont implémentés, avec 12 régressions et probes HTTP/multiprocessus indépendantes. Aucun nouveau bloquant relevé dans ce patch.
- Front : dépendances Vite/esbuild corrigées, audit npm sans vulnérabilité signalée, 17 tests/build et navigateur sur fixtures PASS sous Node 24.22.2 ; revue du lot dépendances PASS. Le parcours navigateur sur les vrais gateway/dashboard est ensuite PASS pour neuf contrôles ciblés (auth, bundle, UI, SSE/reconnexion, export, mémoire seule, déconnexion et fermeture), avec événements QA explicitement déclarés. Les vues workers/relais/usage restent absentes.
- Deux corrections de tests runtime relues : fixture macOS parent canonique et filtre/assertions de session du smoke. Aucun contrôle de confinement relâché.
- Docker Desktop 4.71.0, client 29.2.1/daemon 29.4.1 initialement accessibles ; premier build interrompu RPC EOF. Revenu disponible sans action de redémarrage/configuration de Desktop par les agents : Desktop **4.97.0 (244584)**/daemon **29.9.2**, client29.2.1, `rootless:false`, hôte macOS27.2 ARM64. **PASS réel limité** du confinement : UID/GID, caps, NNP, seccomp, racine readonly, `/run/secrets` absent selon ENOENT strict, réseau none selon routes/connexion testées, allocations cgroup, timeout, copie/export et rejet du callback avec cleanup ciblé. **PASS réel distinct** du smoke via le broker/gateway : UID audité, mutation isolée, export neuf, original intact et refus sans marqueur/conteneur/volume de la session refusée. Saturation, panne daemon, recette trois workers, autres couples et cloud restent non qualifiés. Podman absent ; Kubernetes non implémenté.
- Licence **Apache-2.0** retenue avec latitude donnée par le mainteneur, texte officiel ajouté. Canal privé de vulnérabilités encore à définir. La licence ne clôture pas les exigences de chaîne de livraison.

Le mandat persistant est désormais renforcé et relu dans vingt documents : produit open source complet et soigné, parcours, états UI, accessibilité, exploitation et maintenance ; aucun report d'obligation sous MVP/V2. Les rapports historiques n'ont pas été transformés en résultats nouveaux.

Décisions API idempotentes livrées et relues : request_id/contenu/phase, conflit409, revue durable préalable, faux hash refusé avant cache, mêmes garanties d'audit sur replay, secrets nécessaires à l'identité refusés avant persistance. Neuf tests HTTP et cinq probes indépendantes PASS ; aucune consommation de lancement n'est revendiquée.

**Relais d'entrée Rust : lot clos pour le candidat local nommé.** L'échec historique de publication des services exclusivement internes reste conservé. Après autorisation du mainteneur, un ingress Rust distinct publie les deux accès loopback, sans secrets/volumes, en conservant gateway/dashboard sur le réseau interne. Les deux P1 de disponibilité et deux P2 de revue sont fermés ; les limites de durée/progression, la garde du corps aval et la santé des deux destinations sont revalidées. Sur la pile QA `ai-guard-qa-7dce8f12`, l'image `agentsandbox-control:qa-20261029-8bd72c0e`, ID `sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`, sert gateway `127.0.0.1:48787` et dashboard `127.0.0.1:48400`. Le smoke réel et le navigateur live final réussissent ; leurs FAIL intermédiaires de harnais restent conservés. Ce relais d'entrée ne constitue pas le relais métier inter-workers.

**Terminal runtime du gel 139 : correction logicielle relue.** Running/ExitCode requis, état et dates de fin cohérents ; `exited` admis pour Docker/Podman et `stopped` seulement Podman, code négatif inconnu refusé. La régression compte un test et 36 sous-cas, avec 20 tests ciblés et probes indépendantes PASS. Le scénario Podman utilise un CLI fixture Rust : aucun moteur Podman n'est qualifié. La conservation jusqu'à l'audit terminal, encore ouverte à ce gel, est traitée par le lot P1 suivant décrit ci-dessous.

**Redémarrage gateway à l'arrêt : PASS limité.** Avant/après le redémarrage du seul gateway QA, l'audit reste valide avec **102 événements** et le même head `416c3bc47a40fa592869937efc61a4a0395f56bb3853914105db38b855b668ad` ; les deux JSON sont identiques. L'orchestrateur a constaté une sonde transitoire en échec puis le retour healthy, sans second redémarrage. Les sondes finales `/healthz` réussissent et les trois services utilisent l'image nommée ci-dessus. Un sondage ultérieur de `/health` a produit 404 par erreur de chemin de diagnostic, sans défaut produit démontré. Cet essai sans job actif ne prouve ni crash recovery, ni reprise d'une session ou d'un ledger.

**Sonde ingress multi-destination : PASS réel limité.** Pendant l'arrêt ciblé du seul dashboard QA, le gateway répond encore 200 et la sonde Rust des deux destinations retourne exit 1. Après redémarrage du dashboard, les deux endpoints et la sonde réussissent (exit 0). La pile est rétablie ; aucune reprise de worker n'est déduite.

Les journaux/rapports finaux sont copiés sans écrasement dans [le dossier de preuves local](evidence/2026-10-29/local-final-8bd72c0e/README.md). Les empreintes du candidat gelé concordent avec ses revues finales ; les changements suivants ne sont pas incorporés dans cette image. Aucune qualification globale de release, rootless, Linux x86_64, Podman, Kubernetes, A1/A2/B1, charge 100 ou fournisseur n'en est déduite.

## P1 gelé — logiciel, réel ciblé et rejeux QA PASS au périmètre nommé

**Code gelé par l'orchestrateur : PASS logiciel et revue ciblée finale.** Le relevé final compte **147 PASS/0 FAIL/3 ignorés**, fmt sans écart, Clippy en 2,39 s et release en 17,04 s. Les [preuves du gel logiciel](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/SHA256SUMS.txt) sont distinctes du snapshot intermédiaire 145/release 19,25 s et du candidat contrôle précédent. Les [empreintes de sources et du binaire hôte](evidence/2026-10-29/p1-terminal-authority/software-rootgel-147/SOURCE_SHA256SUMS.txt) concordent au contrôle. La [revue finale indépendante](evidence/2026-10-29/p1-terminal-authority/review-and-real-rootgel/review-runtime-retention-final.md) ne relève plus de P0/P1 bloquant dans le périmètre gelé ; elle relit code et preuves fournies sans exécution propre du reviewer. Les rejeux du nouveau déploiement sont constatés séparément ; aucun succès de l'ancienne image n'est transféré.

**Autorité des nouvelles ingestions : prévention relue au périmètre logiciel.** Les alias d'ingestion refusent les namespaces de décision/contrôle/worker/allocation centraux sans bloquer les traces légitimes comme `usage.reported`. La reproduction exigeait le credential runner du même tenant ; aucun contournement worker ou inter-tenant n'est déduit. Les 21 tests ciblés sont corroborés par la suite intermédiaire et la revue. Le correctif n'authentifie pas rétroactivement les anciennes décisions : un faux événement ancien peut être indiscernable d'une décision légitime. Une future admission doit obtenir une nouvelle autorisation centrale complète, sans transformer un ancien allow en droit start.

**Terminal et cleanup : correctifs logiciels gelés et relus.** Après référence create acquittée inutilisable, aucun token ni fallback par nom n'autorise inspect/start/kill/rm ; les six RED deviennent quatorze sous-cas GREEN au total. Une réponse rm perdue laisse le cleanup `UNCONFIRMED`, sans garantir que le conteneur est encore présent ; `error_audit` distingue `not_applicable`, `acknowledged`, `unacknowledged`. Sans acquittement, aucun helper suivant, `changed_files` inconnu et terminal déjà observé rendu avec CLI en échec. `postprocessing_error` distingue une erreur d'observation ultérieure du résultat métier connu.

**Secrets : derniers RED conservés, correctifs inclus dans les 147 tests.** Une erreur runtime pouvait exposer un secret connu dans la CLI ; tronquer stdout/stderr à 8 192 ou une projection d'adaptateur à 4 096 avant masquage pouvait en conserver un fragment. Le gel masque la chaîne complète d'erreur avant retour CLI et les sorties avant troncature ; le JSON borné à 64 Kio est décodé puis masqué avant projection, y compris les clés d'objet contenant un secret connu. Les neuf cas du harnais CLI et deux tests de frontières JSON sont inclus dans la suite finale, sans afficher les valeurs canaries. Aucun masqueur universel de secret inconnu n'est revendiqué.

**Essai Docker intermédiaire : PASS limité avant les derniers correctifs.** Un test réel passe avec vrai runtime et vrai gateway/SQLite : middleware refusant l'HTTP terminal, instance arrêtée retenue, audit valide et aucune fausse ligne terminale, puis réouverture DB contrôlée dans le second essai. Deux instances/volumes et répertoires distincts sont conservés (`ai-guard-terminal-audit-EJshjl`, `ai-guard-terminal-audit-sYbBpd`). Ce n'est pas une panne physique du disque ; ces essais ne qualifient pas le code corrigé après leur exécution.

**Rejeu Docker du gel final : PASS ciblé distinct.** Le [log final](evidence/2026-10-29/p1-terminal-authority/review-and-real-rootgel/terminal-persistence-real.log) compte un test réel PASS en **1,25 s**, zéro échec/ignoré. Même instance arrêtée conservée après HTTP terminal 503, terminal observé cohérent, aucun faux terminal stocké et intégrité avant/après réouverture SQLite vérifiée dans le test relu. Répertoire `ai-guard-terminal-audit-oViYdH` et instance/volume propres retenus. Le gateway de ce test est un vrai service Rust/SQLite local avec middleware de refus HTTP, pas la nouvelle image conteneurisée ni un disque défaillant. Ni crash actif, ni récupération automatique ou registre ne sont qualifiés.

**Nouvelle image déployée uniquement en QA : rejeux ciblés PASS.** Le build `agentsandbox-control:qa-20261029-security-09c7b861` produit l'ID `sha256:b4239720250bd8a0e05c478d5bd289726041ea753dd701fa4780e795ae9150f2`, Linux ARM64. Gateway/dashboard/ingress sont healthy sur cet ID. Le [smoke déployé](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/deployed-smoke.log) compte un test réel PASS en 3,62 s (original préservé, export et refus sans conteneur/volume), et le [navigateur](evidence/2026-10-29/p1-terminal-authority/deployment-security-09c7b861/deployed-browser.log) neuf contrôles PASS, Node 24.22.2/Chrome 156.0.8078.48. Les événements navigateur restent QA déclarés, aucun effet fournisseur.

**Audit du déploiement et des essais : deux observations distinctes.** Avant/après recréation des services, les JSON sont identiques à 102 événements/même head ; volumes conservés. Après les nouvelles écritures des essais, l'audit est valide avec **137 événements**, head `c876b11546452d5e2567beab4922ad7de506dd4df330d3238a7c99c198390a37`. Cette croissance n'est pas une divergence de conservation. Le lot et les rejeux sont clos uniquement pour Docker Desktop rootful sur macOS ARM64 ; registre, reprise après crash, quotas/consommation, inter-workers, Podman/Kubernetes, charge 100, fournisseurs et facturation restent ouverts. Les [conceptions suivantes](evidence/2026-10-29/p1-terminal-authority/design-next-not-implemented/README.md) sont archivées comme DESIGN, non comme code livré.

**Confinement courant rejoué séparément : PASS limité.** Un test réel PASS en 3,78 s sur le même code gelé vérifie les restrictions/allocations, timeout, réseau none, copie/export et le callback rejeté ; session `cec5c1c4-51ad-4117-89df-9cd4e0a2fe29`, volume et preuves conservés. Les trois tests ignorés de la suite normale ont donc leurs trois exécutions explicites courantes PASS, sans transformer le compte normal 147/3 ignorés. Le [manifest complet de 78 fichiers](evidence/2026-10-29/p1-terminal-authority/final-candidate-coverage/COMPLETE_SOURCE_SHA256SUMS.txt) est vérifié au gel ; il identifie le candidat sans promettre une reconstruction byte-identique des images ou paquets APT.

La consommation durable d'exécution, le registre/quota de workers, le relais **métier inter-workers** et le ledger restent les obligations suivantes. L'identité runtime stable et l'autorité de broker doivent prévenir les reprises ambiguës ; une simple lease SQL ne prouve pas un démarrage exactement une fois.

## Lot courant après le gel 147 — intégration du registre, non qualifiée

La source a évolué depuis l'image QA ci-dessus ; cette image n'incorpore pas les changements suivants. La fondation SQLite v2 et ses contrats Rust sont implémentés et testés au niveau logiciel : admission fraîche, quota transactionnel, claim consommé irréversiblement, observations/allocations exactes non facturables, projections vérifiées et migration avec sauvegarde nouvelle vérifiée. Les nouvelles routes HTTP ont leurs premiers tests A/B, rôles, pagination et quotas ; elles ne constituent pas encore une preuve de lancement de workers.

Une revue indépendante a conduit à reproduire puis corriger un contournement par triggers SQLite aux noms `sqlite_*` : deux réponses de claim fraîches et dépassement d'un quota de un étaient possibles. Les exceptions de schéma sont désormais limitées aux autoindexes attendus et les projections sont vérifiées après la dernière écriture. Les régressions et la relecture sont conservées dans les rapports temporaires du lot ; leur archivage final attend le gel intégré.

Le nouvel adaptateur gère les identités et noms attribués par le contrôle, l'image immuable, les inspections et les callbacks avant start ; ses fixtures ne qualifient pas un moteur. Les vues Workers/Quotas/Allocations passent leurs tests frontend et navigateur sur fixtures, avec une correction de retour du focus après rafraîchissement. Le broker, les checkpoints, les commandes de reprise/arrêt et les harnais sont encore en intégration. **Aucun PASS de la précédente image ni des trois anciens essais moteur ne qualifie ce parcours nouveau.** Le relais métier, la restauration v2 sûre, les pannes actives, le contrôle de durée après crash du broker, Podman/Kubernetes et la charge 100 restent ouverts.

### Progrès du même lot — preuves intermédiaires distinctes

Le broker courant utilise le registre v2 pour `run`, le verrou OS par tentative, les checkpoints privés, `worker-status`, `worker-resume`, `worker-stop` et l'export lié à l'autorité et au tenant. Les tests CLI/HTTP/SQLite avec moteur **fixture Rust** couvrent les accusés perdus à create/Created/start, la contention du verrou, l'arrêt demandé pendant une session et les refus d'origine/tenant/binding/manifeste/archive. Ils ne prouvent pas un crash sur Docker réel. Les reprises avant création et après create sans accusé sont étroites ; elles ne réarment aucun claim de start.

**Nouveaux essais Docker réels : PASS ciblés intermédiaires.** Un gateway v2 privé neuf sur loopback, distinct de l'image QA 147, a servi le smoke positif : **1 PASS en 10,82 s**, session id/UID1000, refus sans conteneur, copie originale préservée et export neuf filtré. Le test vérifie aussi un worker terminal, son allocation observée `billable=false` et deux reprises après cleanup qui conservent exactement une allocation. Deux volumes et les sources/exports sont conservés. Un autre essai, **1 PASS en 3,68 s**, refuse l'observation terminale HTTP avant Store : même instance arrêtée conservée, audit valide après réouverture, absence d'allocation et quota inconnu tenu. Logs et portée dans le [dossier du lot](evidence/2026-10-29/worker-registry/README.md). Ces essais ne sont pas un nouveau déploiement conteneurisé, une recette trois workers ou une panne disque.

**Dashboard partagé : implémenté, intégration réelle non qualifiée.** Le mode explicite `--shared-tenants` vérifie le viewer auprès de l'identité gateway ; le mode viewer fixe reste compatible et refuse aussi une clé runner mal configurée. Les tests HTTP réels A/B et CAS/rôles passent. Le front lit l'identité avant les données, affiche son tenant et reste utilisable avec un overview indisponible. **27 tests Node et le navigateur sur fixtures passent**, dont bascule A→B avec douze continuations A retardées, vieux refus403, péremption limitée à B et identifiant long sur mobile. Aucune fuite antérieure A→B n'a été reproduite : les gardes existantes sont conservées et vérifiées.

**Revue intermédiaire : intégrité du replay corrigée au gel suivant.** Le second regard a identifié une validation incomplète des deltas pour les actions autres que `prepare`/`created`, sous précondition de réécriture externe de la base **et** recalcul de sa chaîne/projection. Aucun accès distant ou worker à cette réécriture n'est démontré. Le correctif du gel ci-dessous partage la logique de mutation/rejeu de toutes les actions, reconstitue leurs inputs et recoupe le reçu d'admission avec sa décision antérieure. Les RED, GREEN et relectures sont conservés.

### Gel intégré 209 et nouveau déploiement QA

Le [dossier de preuve](evidence/2026-10-29/worker-registry/README.md) contient **209 tests Rust PASS, zéro échec, trois tests moteurs ignorés**, fmt/Clippy/release PASS, ainsi que les 27 tests Node/build/navigateur sur fixtures. Le manifest de 90 entrées source est vérifié. Les défauts reproduits du replay et du résultat CLI de reprise sont corrigés et relus. Une commande reprise échouant avec exit7 conserve son JSON et retourne désormais un échec, y compris après lecture du résultat sauvegardé.

L'image Linux ARM64 `agentsandbox-control:qa-20261029-workers-31d2c5af`, ID `sha256:39111cab0564e8aa6a11f07772df6c1824bd61c86e1ed25a6b4864b1e17805d8`, est déployée sur la pile QA existante. Migration offline v1→v2 réelle avec sauvegarde neuve, sans suppression : les **137 événements antérieurs** précèdent le genesis138. L'audit est valide à138 après migration, puis à**195** après les essais, head `536c2c9d9678de8e75ad256d1e47823547dbd37229f115af68d655c49ba94113`.

Les trois tests ignorés sont exécutés séparément sur Docker Desktop rootful/macOS ARM64 : confinement **1 PASS/3,75s**, refus terminal HTTP avant Store **1 PASS/2,75s**, smoke sur le nouveau contrôle conteneurisé **1 PASS/10,23s**. Le smoke vérifie une allocation unique après deux reprises et l'inventaire réel de volumes inchangé lors du refus, sans calculer un ancien nom de volume. Le navigateur réel passe **10 contrôles en mode fixe**, puis **10 en mode partagé** : identité, bundle, autorisation, SSE/reprise/export et vues workers/quota/allocation liées au worker explicite du smoke. Le dashboard partagé n'a aucun montage, notamment aucun fichier de secret.

Ces preuves concernent un tenant QA local. Le routage A/B des API est testé logiciellement, pas encore une pile conteneurisée A1/A2/B1 complète. Le crash/reprise du broker est exercé par harnais CLI/HTTP/SQLite avec moteur fixture, pas encore qualifié sur moteur réel. Le contrôle de durée autonome, la restauration sûre des claims, le relais, Podman/Kubernetes, capacité100, fournisseurs et chaîne de livraison restent ouverts. La pile QA reste active ; sa configuration courante est `/tmp/ai-guard-registry-qa.40IImn/compose-shared.yaml`, projet `ai-guard-qa-7dce8f12`, configuration privée conservée à son ancien emplacement. Ne pas réutiliser l'ancien Compose147 pour une mise à jour du schéma v2.

### Audit indépendant reçu après ce gel

La revue externe porte sur un autre instantané, `/private/tmp/ai-guard-audit-20261029.n8ZxPB` :169 fichiers, manifest annoncé `77876e6de40964096f90f458b07b872b7848f1a05257e2f763e5f57679641d34`. Son résultat formel reste **0 confirmé, 19 besoins de validation**, sans exécution cible faute de sandbox conforme. Il ne qualifie pas le code209, et les tests209 ne réfutent pas automatiquement ses candidats. Leur rapprochement différentiel et les corrections prioritaires sont suivis dans [AUDIT_FOLLOWUP.md](AUDIT_FOLLOWUP.md). L'objectif produit demeure actif.

Les correctifs de développement capture/masquage et collision Jev/contrôle sont désormais intégrés au [gel235](evidence/2026-10-29/audit-followup/README.md), distinct du209 : fmt/Clippy/suite/release PASS,235 tests/0FAIL/3ignorés, revue indépendante traitée. Les secrets littéraux coupés par capture sont masqués avant décodage, les JSONL complets conservés et les erreurs CLI typées gardent leur troncature et la protection des marqueurs privés. Jev est séparé de toutes les identités de contrôle à init/startup et à l'assemblage de la bibliothèque.

L'image QA active est `qa-20261029-secrets-a64d7b0e`, ID `sha256:4eb348a836fe51ae3325aa5c078a98a505ab0dc87306762f35b1faf567c12a78`, trois services healthy ; Compose actif `/tmp/ai-guard-registry-qa.40IImn/compose-secrets.yaml`. Audit270 identique avant/après mise à jour, puis347 valide après essais. Confinement4,14s, refus terminal3,13s, smoke déployé22,24s et navigateur10 contrôles PASS. Le smoke ajoute une valeur synthétique longue réellement émise par un worker sur stdout/stderr et vérifie son masquage durable ; profil observe/bridge explicite, sans appel fournisseur. Le test de protection des marqueurs au bootstrap utilise une fixture Rust, pas Podman réel.

Au gel235, le choix fiable du programme moteur et l'ancrage filesystem étaient les prochaines priorités. Aucun service de supervision autonome n'est installé. Les19 verdicts de l'audit original restent `needs_validation`. La direction Nothin’ est inscrite dans AGENTS.md et docs/ui.md ; la déclinaison visuelle dans l'application reste un chantier ouvert, avec les critères d'accessibilité et de supervision.

### Gel261 — invocation du moteur et compatibilité des sessions

Le [dossier du lot](evidence/2026-10-29/trusted-invocation/README.md) identifie le code relu et vérifié : fmt/Clippy all-targets/release PASS, **261 tests Rust exécutés/0FAIL/3ignorés** et manifest de97 fichiers vérifié. Aucun PASS du gel235 n'est transféré. Programme hors PATH ambiant et projet, politique2 avec FD du répertoire final, parcours canonique sans liens, owner/modes/ACL, lecture hash bornée et environnement reconstruit sont livrés. Les commandes Docker d'administration gardent la même cible Unix et le même EngineID entre diagnostic et effets.

Les anciennes sessions restent consultables. L'adoption explicite d'une invocation absente, policy1 ou d'un client remplacé vérifie en lecture seule la même autorité, conserve l'historique privé et exige audit stable puis checkpoint avant effet. Aucun claim n'est réarmé. Les défauts FIFO, projet d'origine déplacé, substitution de parent, lien lors du pinning et repli après échec d'audit ont leurs régressions ; le second regard ne relève aucun bloqueur concret restant dans ce périmètre.

Image QA active `agentsandbox-control:qa-20261029-invocation-bd32c681`, ID `sha256:78d835562124c9d46c569c9efb715b7c35b2e44730780832ce14891a23efbb98`, trois services healthy. Compose courant `/tmp/ai-guard-registry-qa.40IImn/compose-invocation.yaml`, projet et volumes conservés, aucun changement de schéma. L'audit347/même head est identique avant/après remplacement des services. Après les essais : **512 événements**, head `5a13a49af8594eb38ef6c64c7e21106b174d352d66620eef869388f02078d13c`, valide.

Docker Desktop rootful/macOS ARM64, exception explicite : trois tests ignorés exécutés séparément PASS. Debug : confinement124,98s, terminal86,95s, smoke449s. Release : confinement11,04s, terminal8,61s, smoke45,19s. Des essais ont été concurrents ; ces durées ne constituent pas un benchmark isolé. Le coût debug observé reste important. Le navigateur réel partagé passe10 contrôles sur le nouveau worker release ; aucun redesign livré. Une ancienne session235 a aussi passé refus sans adoption, adoption et deux exports neufs, avec worker/instance/claims inchangés et un seul événement d'adoption.

Le choix du programme est corrigé au périmètre nommé ; Podman réel, broker Linux, liaison opérateur Podman, plugins/dépendances du contrôle, déplacements administratifs d'arbres et fencing moteur restent hors preuve. Le [vertical filesystem suivant](evidence/2026-10-29/trusted-invocation/filesystem-next-design.md) est conçu, non implémenté : configuration et snapshots de credentials remis aux CLI, puis SQLite/WAL/backup. Durée autonome, relais, recette A1/A2/B1, restauration, quotas disque/logs, capacité100, cloud et publication restent ouverts. La qualification production globale n'est pas acquise.

### 30 octobre — chantier filesystem en intégration

La source évolue après le gel261 ; l'image QA reste celle du gel261. Une copie des97 sources qualifiées et du binaire a été conservée à `/tmp/ai-guard-invocation-freeze.iJpHZL` avant ces nouvelles modifications. Son archive source SHA256 vaut `701934bf1d20977053e4f9495615e6ce32df835bfa2827804a2a2f7a8d4d1371`. Le manifest261 ne prétend pas qualifier les changements suivants.

La primitive Rust `control_fs` est implémentée et en revue ciblée : chaîne de parents protégée par ownership/modes/ACL, fichiers privés et snapshots partagés, bytes/digest stables et nettoyage relatif aux FD. Son intégration dans le broker, les credentials et le handoff Compose est en cours. Les vérifications ciblées du sous-lot ne constituent pas encore une validation globale du nouveau parcours. SQLite/WAL/backup reste le lot obligatoire suivant ; aucune nouvelle correction de sa course d'ouverture n'est revendiquée.

### Gel302 et refonte éditoriale — déployés en QA

Le [lot filesystem](evidence/2026-10-30/control-filesystem/README.md) est intégré et relu : fmt, Clippy all-targets, release et **302tests Rust/0FAIL/3ignorés PASS**. Sources de credentials et snapshots partagent les bytes/digests préparés ; configuration retenue par FD et exclusion physique de toute sa racine lors de l'import, y compris noms personnalisés/alias de casse. Les anciens snapshots reconnus sont filtrés. Une archive historique sans preuve ne reçoit aucun premier start ; les claims consommés ne sont pas réarmés. Scan readonly sans dépendance au projet original et gardes d'export conservées. Les défauts de revue sont traités, notamment ownership ignoré sur Darwin, ACL, casefold et fuite d'import du contrôle personnalisé.

Sous Linux ARM64 en conteneur de test UID1000 :3unités/13intégrations PASS, dont ACL POSIX réelle, sources identiques au gel. Il s'agit de tests filesystem sur le noyau de la VM, pas d'une qualification de broker Linux pilotant un moteur. Podman, vrais volumes noowners, autres filesystems et multi-UID restent non qualifiés.

La [refonte demandée](evidence/2026-10-30/editorial-ui/README.md) est réellement livrée : header clair, navigation sombre fixe, neuf vues numérotées, grands titres, base blanc cassé/noir et Suivant citron. L'authentification mobile est visible dans le premier viewport à320px. Node27tests/build/navigateur,45couples vue/viewport sur le shell, revue indépendante et derniers contrôles login PASS. Le navigateur sur vrais services Rust passe10contrôles ; captures desktop/mobile réelles ouvertes. Le français est conservé, aucune traduction anglaise ni certification WCAG complète revendiquée.

Image active `agentsandbox-control:qa-20261030-editorial-fs-6f4d8b02`, ID `sha256:d8179e589f7104072f2b237860d809a254acaf474a7593b380f0a91cdf52ca7f`, gateway/dashboard/ingress healthy. Compose courant `/tmp/ai-guard-registry-qa.40IImn/compose-editorial-fs.yaml`. Audit512 identique avant/après remplacement ; après essais589événements, head `a1a2d354f4d9a712d4792474b39abf583e6c0882e56e3ad25ba9752150a71bb4`, valide. Volumes, données et anciennes images conservés. Docker Desktop/macOS ARM64 rootful explicitement choisi : confinement release11,84s, refus ACK terminal10,02s et smoke déployé42,04s PASS ; aucun compte fournisseur réel.

**Prochain P0 : SQLite/WAL/SHM/backup.** La nouvelle primitive n'y est pas encore appliquée. Puis supervision autonome, réconciliation/fencing, relais et recette A1/A2/B1, quotas disque/logs, capacité100 et cloud/Kubernetes restent obligatoires. Les19 verdicts de l'audit figé restent inchangés ; le produit n'est pas globalement qualifié production.
