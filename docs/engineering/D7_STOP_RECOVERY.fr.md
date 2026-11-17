# D7 : propriétaires STOP natifs de reprise

[English](D7_STOP_RECOVERY.md). Candidat du 17 novembre 2026. Compilation,
régressions logicielles et revue indépendante de ce candidat restent
**NOT_RUN** jusqu'à leurs preuves distinctes. Ce document ne qualifie ni
moteur, ni échéance physique, ni gate de production.

Le registre accepte `max_recovery_stop_owners` (`W`, défaut `0`, plage
`0..=1024`) indépendamment de `max_enforcer_slots` (`S`, inchangé). L'enforcer
privé reçoit `--max-recovery-stop-owners`. W ajoute des ressources STOP, jamais
de capacité Original/start. Pour W>0, une cellule est réservée au rattrapage
avant publication de l'endpoint ; W−1 cellules gardent des propriétaires STOP
permanents supplémentaires. Capacité permanente : **S + max(W−1, 0)**, dans
un budget natif total S+W. S=1/W=2 possède deux cibles et garde une cellule
de rattrapage. S=1/W=1 possède une cible et tente le surplus. W=0 ne fournit
aucune ressource de rattrapage.

Chaque cible acceptée conserve son snapshot immuable vérifié en RAM,
identité/échéance originales, dernier Cancel admis, lease native du rôle,
vrai handle du thread et contexte runtime construit. `StopReserved` suit
l'installation ; un contexte non construit ne reçoit aucun ACK de couverture
active. Ni erreur moteur ni observation d'arrêt ne retirent un propriétaire.
Ces cellules ne créent ni nouvelle Arm, nonce Original, Q/R+, éviction,
ni libération de quota.

La sélection STOP ne lit aucun catalogue de découverte et ne prend aucun
mutex writer/HTTP. Les contrôles natifs d'exécutable/endpoint et de snapshot
exact restent. Une IO runtime bloquée conserve cellule et handle, sans
remplacements illimités. Le rattrapage garde au plus le snapshot en cours
et un snapshot en attente ; son vrai futur runtime doit revenir avant
qu'une autre tentative utilise cette cellule.

Chaque cellule native conserve la véritable première erreur et la dernière
erreur positivement collectée, avec leurs causes typées et captures. Les
anciennes erreurs collectées peuvent être omises ; un compteur vérifié rend
ces omissions explicites. Une troisième position garde le véritable appel
non collecté, sa cause originale, Child/pipes, captures partielles et handles.
Ni chaîne de caractères, panic de tâche, downcast absent, notification perdue
ni timeout ne certifie `NoChild` ou `Collected`. Seuls les producteurs réels
de spawn/collecte émettent ces preuves opaques. Une cellule avec IO pending
ou inconnue ne peut commencer une autre tentative ni rejoindre la retraite.
Une panne ordinaire collectée permet au même contexte de réessayer et au
rattrapage de passer équitablement à une autre cible. Son historique garde
la demande exacte à côté de chaque cause retenue. Outre les snapshots en cours
et en attente, au plus deux demandes liées aux causes collectées restent
retenues, avec des captures bornées. Aucun autre budget de cellule, thread
ou runtime natif n'est ajouté.
L'unique payload de panic interceptée du thread natif prospectif/liée reste
aussi conservé comme objet opaque original ; il rend la custody explicitement
inconnue et ne permet ni remplacement d'invocation ni Q.

Les vues prospective et Full liée partagent le même propriétaire de causes.
Un kill en erreur suivi d'une observation exacte d'arrêt conserve les deux
faits ; un suivi en erreur garde séparément la cause du kill et celle du
suivi. Les wrappers publics Result du runtime conservent leur comportement ;
le port natif crate-private fournit ce résultat enrichi aux consommateurs D7.

Un surplus reste `RecoveryFenced/Incomplete`. La cellule préinstallée effectue
de vraies tentatives reopen/STOP exactes. Le cache de contrôle conserve les
preuves de source et reprend les cibles éligibles par balayage circulaire
borné. Une tentative ne donne ni ACK de couverture active ni retraite et
ne draine pas un ancien start. `recovery_attempts` compte les tentatives
revenues, y compris le contrôle concluant qu'aucun arrêt n'était dû ;
`recovery_attempt_failures` conserve les résultats inconnus/en erreur.
Une erreur moteur ne compte jamais comme une observation d'arrêt.
`recovery_attempt_in_flight` reste vrai tant que le véritable appel natif
n'est pas revenu ; un timeout de transport ne libère ni snapshot ni cellule.

La rétention de découverte est bornée séparément à 16 384 cibles distinctes
par client STOP. Preuves et requêtes pending sont des données, pas des
propriétaires actifs. Une source hors limite, incomplète, divergente ou
indisponible laisse la reprise incomplète, sans éviction. Rapport eager,
transfert sérialisé, requêtes retenues et snapshots actifs comptent tous
dans la mémoire : aucune garantie de mémoire constante pour un historique
arbitraire. Chaque cellule demande une pile de 1 Mio, plus ses véritables
ressources runtime, IO, snapshot et transfert. Mémoire physique/FD et latence
restent à mesurer sur la plateforme choisie.
Une cellule S prospective conserve son snapshot original et la registration
liée au vrai reçu : au plus deux contenus de snapshot par cellule S, avec
le même unique thread/runtime natif, jusqu'à sa véritable sortie.

`BeginRecovery`, authentifié, confirme le gate natif partagé par toutes les
vues Full de la lease. Le rôle froid est fenced avant le scan ; une Full
publiée plus tard hérite du gate. Finalisation Register et Prepare le
revérifient. STOP/Cancel existants et continuations de règlement déjà
autorisées gardent leurs contrôles. Un ancien daemon incapable de confirmer
cette opération est refusé, sans takeover, kill ou migration déduite.
La voie Legacy réellement neuve sélectionnée par défaut dispose désormais de
sa propre factory native dans le même binaire Rust. L'enfant crée rôle et lock
V1 seulement par mkdir/create exclusifs réussis et installe les vrais supports
et ressources S/W. Aucun ledger Full3 ni droit de retraite n'est créé. Un pipe
stdin privé corrèle le parent qui a réellement lancé cet enfant au nonce et
à l'epoch authentifiés de la factory. Le nonce seul est DATA, sans permission.
Seul ce parent garde la provenance Fresh ; aucune reconnexion ne la reconstruit
depuis un status, nonce, timeout ou rôle paraissant vide.

La reprise initiale est sautée seulement avec cette provenance native, des
éléments de contrôle complets et un inventaire journal strictement absent/vide.
Toute source antérieure ou inconnue confirme BeginRecovery Legacy avant scan.
Les rôles existing/cold sont fenced avant publication. Un vieux daemon vivant
sans nouvelle capacité prospective authentifiée refuse une nouvelle Arm et un
nouveau scan de reprise ; ses anciens chemins Status/Slots/Cancel/STOP et
owners natifs restent. Ni kill, takeover, rekey, adoption de répertoire ni
conversion Full3 silencieuse.

Quand le premier gate de start dans la préparation Submit refuse, le broker
tente un unique diagnostic JSON sur stderr, au plus 256 octets avec le saut de
ligne : `version=1`, `component=broker`, `event=launch_admission_refused`,
`stage=start_gate`, l'`operation_id` exact et le `BrokerErrorCodeV1` sérialisé
fermé dans `reason_code`. Aucune erreur libre, credential, profil, commande ni
chemin n'y figure. Une écriture en erreur ou courte peut omettre ce diagnostic ;
elle ne change aucun résultat et ne constitue pas un audit durable. Le rejet
durable existant fait foi. Sous fence, la projection Status peut répondre
`Unavailable` sans exposer la cause de préparation locale au processus.
La borne porte sur les octets et l'unique essai, sans garantie de latence de
l'écriture stderr synchrone ; ce diagnostic best-effort ne prouve ni audit
durable ni STOP.

La séquence pré-Arm persiste la demande Pending exacte, réserve son owner
natif S, puis consomme le grant natif unique d'échange Arm avant la RPC.
Un refus de capacité après Pending mais avant installation conserve la même
identité/échéance. Une reprise sans owner natif refuse de recréer Original
depuis ces données. La disponibilité de reprise de ce cas reste un écart
ouvert ; aucun cleanup synthétique ou quota libéré ne le clôture.

Le status distingue S/W, cellules de rattrapage, propriétaires actifs installés,
fence/reprise incomplète et tentatives revenues. Santé du transport et comptes
ne prouvent ni couverture exhaustive de source, conteneurs arrêtés, drain
d'ancien start ni déploiement qualifié. Les régressions à deux cibles gardent
le contrôle cap2 et S=1 inchangé avec W=2 explicite ; le cas W=1 exige une
vraie tentative de rattrapage et un résultat incomplet avec un propriétaire
actif. Leur runtime indisponible ne démontre aucun effet conteneur. Les
résultats restent liés à leur source composée et leur exécutable exacts.

Le status STOP natif authentifié expose aussi `retained_native_causes`,
`omitted_settled_causes`, `cause_omissions_exact` et `native_io_uncollected`.
Ces diagnostics bornés ne donnent aucun droit Arm, Q, terminal ou quota et
ne contiennent aucune capture. L'absence des champs chez un ancien pair
n'accorde aucune couverture exacte des omissions. Seul le status de transport
natif est étendu ; les records publics de reprise du journal sont inchangés.
Les régressions de collecte/causes utilisent de vrais Child/pipes logiciels
et les propriétaires natifs, avec Ready/reçus synthétiques lorsque précisé.
Leurs sources restent **NOT_RUN** jusqu'à la validation Luna de la composition
gelée. La composition ports/native Legacy demande sa propre revue et ses
tests ; aucun ancien résultat ne la qualifie. Reprise de capacité après Pending
sans owner et réouverture métier automatique d'un rôle froid restent ouvertes
avec les critères de reprise ci-dessus.

Legacy négocie une réponse fermée distincte : version, epoch, S/W, génération
de reprise, corrélation de lancement, cibles connues observées, `owned_active`
(null explicite si inconnu) et caractère incomplet. Le status V1 ne gagne aucun
champ : son compte et Shutdown utilisent le même registre natif incluant les
prospectifs. Les données de reprise du client gardent au plus 16 384
registrations, sans allouer d'owner ni acquitter le rattrapage. Begin consomme
l'échange natif unique ; le client prend son nonce avant connect/write.
Register générique, perte de Begin/Bind et rejeu de reçu ne recréent pas
Original. Assignation/préclaim/Q Full3 restent propres à Full3.

Pour un ancien rôle vivant, garder la connexion de contrôle existante et
utiliser lorsque disponible le Cancel normal de l'opération exacte avec UUID
de commande stable. Examiner status, preuves de reprise et observations runtime
réelles ; le journal ne prouve pas un drain physique. Source/fence indisponible
reste inconnu, jamais zéro activité. Récupérer auth/ancres originales par
maintenance contrôlée ; ne pas supprimer un rôle pour forcer Fresh. Ce lot ne
fournit ni drain automatique, retraite/quota Legacy, réouverture du gate ni
migration qualifiée. Claim/start exige toujours la vraie voie ManagedRuntime/
Ready et une campagne moteur autorisée distincte. Les nouvelles fixtures
logicielles Legacy emploient vrais owners, contextes Tokio et IPC avec
invocation indisponible, sans SyntheticReady, effet moteur ou succès Claim/start.
