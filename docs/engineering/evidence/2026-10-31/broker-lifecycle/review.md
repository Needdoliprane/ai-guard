# Revue indépendante — lifecycle du broker et admission supervisée

Verdict actuel : **CLOSE logiciel et couverture ciblee**, aux gels finaux cites en fin de rapport. Documents et artefacts finaux restent une revue distincte. Les sections GEL1 et intermediaires conservent la chronologie des constats ; le verdict final ci-dessous les remplace.

31 octobre2026. Worktree broker-service, base `4890df13f56f2a61d9ee773f9d1dbd7cca929e72`. Contrat : dernière section canonique BROKER_JOURNAL. Sources GEL1 vérifiées contre le rapport propriétaire `/tmp/ai-guard-broker-lifecycle-implementation.md` et archive `/tmp/ai-guard-broker-lifecycle-gel1.6aXQOu`. Lecture seule ; aucun Cargo, test, moteur, modèle ou source du dépôt modifié par le reviewer.

## Gel examiné

- lifecycle.rs `d0298b9c322b26a9de0b9b10d91fde55987376625930f445b6aa4810ae663aef`
- journal.rs `6cf5a6a5481315bdaa95a62a3e5cae2f84848010e25de3bdd8e2fc2c7b579bd9`
- journal/types.rs `a6f70ad3e65a004deaac75d29ae70cfe109229a658a91a7a82f219095ec6fd78`
- checkpoint.rs `2319c6a3867496f0fbf90b7fe97b8992b9c300d23bd38917e46f075d2bf8f104` : visibilité crate de has_response seulement.
- checkpoint/validation.rs `5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15` inchangé.
- broker/mod.rs `51012997653d6f4c53d25de956a8a8d718ca14bc93e3000a58898a182a073504`.

## BLC-01 — grant Export antérieur au cancel réutilisable pour un nouveau claim

Confirmé par inspection, reproduction Luna attendue ; root retient le correctif contractuel. `lifecycle.rs:386–429` autorise les nouvelles préparations, Arm et HelperClaim de purpose Export sans retrouver leur date de naissance relative au dernier CancelRequested. V2 retrouve bien un grant exact valide, mais accepte un ancien ExportRequest ACK antérieur au cancel. Scénario : Worker Terminal, ExportRequest ACK puis préparation/Arm Export avant cancel ; ajouter CancelRequested et ensuite un nouveau HelperClaim utilise ce grant antérieur. Aucun nouvel ExportRequest postmortem n'est nécessaire au GEL1.

Root confirme : nouveau start Export exige une ExportRequest née dans une révision STRICTEMENT postérieure au dernier CancelRequested confirmé, puis ACK. Un ACK tardif ou co-né ne rafraîchit pas une demande ; plusieurs cancels peuvent invalider successivement les exports. La frontière est dérivable sans wire nouveau en retenant le nombre d'opérations de la révision de naissance de chaque cancel. Une Export co-née a un index inférieur à cette frontière et n'est pas éligible. Vérifier le grant sur nouvelle préparation Export, nouvel Arm ET nouveau HelperClaim : un ancien Arm ne suffit pas. Loader de tout le préfixe et advance doivent partager cette dérivation. Les réponses tardives restent enregistrables, sans permission de start. Ceci est un défaut causal de données, pas une vulnérabilité moteur/service livré.

## BLC-02 — IDs Reserved/Interrupted déjà observés non revalidés hors page

Confirmé par inspection, reproduction Luna attendue ; root confirme que le contrat inclut ces statuts. `journal.rs:685–710` ne revalide avant pagination que les clés floors et rejected_floors. `status_locked` n'enregistre pas un job observé comme ReservedInProcess ou Interrupted dans ces maps. Cas : status(B)=Interrupted, modifier reservation.json de B en JSON corrompu, puis list(limit1) sur A<B. L'inventaire types/UUID demeure valide et le corps B n'est pas relu ; la page peut réussir malgré ce job déjà observé hors page.

Correction retenue : ensemble d'IDs observés partagé, borné aux caps, revalidation hors page et disparition/corruption explicites. Ne pas confondre cet ensemble/plancher mémoire avec un cache de corps FS ou un anti-restore après restart. Le RED préexistant reserve accepté→Interrupted après disparition accepted.json est, lui, fermé à la source au GEL1 par le chemin status_locked et les vérifications de planchers avant retours non-accepted.

## BLC-03 — candidat de diagnostic public des enums de rejet

Confirmé par RED Luna au GEL1 ; corrigé en source au GEL2 (voir réexamen ci-dessous). `journal/types.rs:494–505` dérive directement Deserialize pour RejectionPhaseV1/RejectionCodeV1, tandis que LocalFailure* remappe ses erreurs de Wire en message opaque. Un input enum string inconnu peut donc être reflété dans l'erreur serde publique. Oracle demandé : erreur de serde_json::from_str::<RejectionCodeV1> sur une valeur canari inconnue et assertion d'absence du canari. La lecture des records Journal remappe déjà ces erreurs en JournalError opaque : aucune fuite d'un caller produit n'est démontrée. Si le trait Deserialize public est couvert par l'exigence opaque, appliquer le même remappage que les autres enums après RED.

## Propriétés recoupées et limites

Hash des commandes recalculé dans le vrai contexte opération/intent ; listes append-only, unicité UUID, bornes et approbation en deux commits. Grand-père V1/V2 approuvé, naissance pending V2 et checks causaux existants conservés. V3 utilise domaine distinct et publication/CAS/replay communs ; rev0 imposée V1, downgrade refusé, reçu de commande dérivé de la première présence confirmée. Fichiers rejet canoniques/exclusifs, dual marker refusé, sync fichier/job/jobs avant reçu ; planchers accepted/rejected consultés avant absence/Interrupted/rejet. Pas de lock récursif apparent dans les lecteurs. Code Store/reducer et validation V2 inchangés.

Verdict intermédiaire : BLC-01/02 ouverts, BLC-03 candidat ; pas d'approbation automatique du gel. Reproduction/correction/qualification finale encore attendues. Created peut rester un fait tardif sous CreateIssued antérieur ; ce fait ne permet pas de nouveau create. Preparing sans CID conserve quota et besoin de réconciliation. Service/IPC/enforcer, authenticité centrale, derniers gardes mémoire, D5, OS/cloud et production restent hors clôture.

## Réexamen GEL2 et raccord HTTP — revue source

Copies GEL2 relues, SHA conformes : lifecycle c6b75bd0c7eba175cf1c1bfaa5d5095cae939fc775f44eca97bca22abbb09e7f ; journal ead8b0726fd18c96df8f936027d7f33ac121d5a2f12289144a6a17c18f07f6b3 ; types172f76f83dc262ea90bc4ad21ff4179da6d1b4a08e4cae7582abc3e25236bac3 ; trois autres sources du gel inchangées. Nouveau runner root dc7cd4a95a8e9eeed4c4a32425e74ee6b96115d92e894bf17c389005e8e3a0d9. Aucun Cargo/runtime par le reviewer.

**BLC-01 fermé en source.** Le loader dérive last_cancel_operations sur chaque naissance Cancel et le passe à la validation d'advance. La nouvelle frontière égale la longueur d'opérations de cette révision ; nouvelle Export co-née refusée. Les helpers Export nouveaux, Arm Export et HelperClaim Export retrouvent un grant dont l'opération est postérieure à cette frontière. L'ancien Arm ne suffit plus. ACK tardif reste enregistré sans rafraîchir son index de naissance. Les frontières successives sont retenues sur tout le préfixe, sans wire libre ni historique tronqué. Created/owned tardifs exigent CreateIssued antérieur, sans nouveau create permis. RED lu dans gel1-export-cancel-release.log : vraie variante graphe Store, assertion refus d'ancien grant échouée, RC101.

**BLC-02 fermé en source.** observed_jobs HashSet partagé reçoit New, Reserved, Interrupted, Accepted et Rejected effectivement observés ; None n'y entre pas. Cap égal au cap jobs, vérifié avant insertion nouvelle. List revalide l'union dédupliquée observed/floors/rejected_floors avant pagination ; reserve/status/load refusent aussi disparition connue. Pas de cache des corps ni anti-restore restart. RED lu dans list-observed-outside-page-diagnostic.log : list de A réussissait malgré la réservation B connue corrompue hors page, RC101.

**BLC-03 fermé en source.** Les deux enums Rejection gardent leur wire mais interceptent désormais toute erreur Wire avec un message statique opaque. RED lu dans rejection-enum-red.log : assertion de non-réflexion du canari phase échouée, RC101. Aucun caller produit exposant la valeur n'est inventé ; le parse Journal était déjà opaque. Les GREEN finaux restent attendus.

### BLC-04 — Worker initial HTTP insuffisamment validé

Confirmé par inspection et retenu par root, RED technique en attente. La nouvelle AuditClient::admit_supervised vérifie statut201/200, flags replay/acceptation, reçu/input_hash/audit, Reserved/rev1/quota, launch/spec et receipt de décision. Mais runner initial dc7cd4a9 ne vérifie pas worker_id/attempt_id UUID canoniques non nil et laisse container_id, created_inspection, create/start_operation/emitter, observed_started_at/last_observation, unknown/cleanup et allocation_inconsistent libres. Un vrai DTO201 altéré uniquement par attempt_id=nil ou start_operation_id=Some(UUID) reste Ok malgré la promesse d'état initial. Le vrai producteur audit/control.rs:742–780 mint les UUID et initialise tous ces faits à None/false.

Ce n'est ni un droit de start ni une violation tenant démontrée ; le journal/contexte central restent requis. C'est un écart de validation du nouveau raccord, à corriger à sa frontière. Root retient UUID/formes et état initial exhaustif, les noms physiques/PolicyHash restant des données serveur à recouper avec namespace/identité côté service/journal. Luna reproduit par middleware loopback altérant un vrai DTO Store avant correction ; aucun moteur nécessaire.

Autres points HTTP relus : méthode nouvelle seule, endpoint existant et token explicite, même reqwest timeout15s/connect3s/no-proxy/no-redirect, cap64KiB requête et512KiB réponse, erreurs opaque/EOF/champs inconnus, 403Policy/429Quota conservés avec reçu et worker/révision absents. Autres statuts, auth/503 ou corps invalide restent erreurs inconnues, sans faux refus durable. Aucune persistance ou autorité de start fournie par ce client.

Verdict source au GEL2 : BLC-01/02/03 corrigés ; BLC-04 ouvert avant correction et preuves. Aucun autre blocage confirmé dans les deltas lus. Qualification des tests/canaris et documentation finale restent attendues.

## Reexamen BLC-04 - runner corrige apres RED

SHA runner constate `d559105e83f755bffbe9d749b7b2f03447a4437ebe3fb456e351d75052d402ce` ; trois sources core GEL2 toujours c6b75bd0/ead8b072/172f76f8. Aucun Cargo par le reviewer.

RED lu : `/tmp/ai-guard-broker-lifecycle-final/initial-worker-response-integrity-red.log`, RC101 ; le vrai DTO201 est accepte malgre start_operation_id ajoute. Test source actuel fcdce7ae6222757b16e125c96fd37082905a256e6f82ad1654a5d685a851de18 relu : middleware execute apres le vrai handler et Store, modifie uniquement start_operation_id, puis l'oracle exige Err et verifie que l'operation existe reellement en Store. La reponse de test n'est pas un Worker invente ; son alteration reste explicitement technique et ne qualifie aucune autorite externe.

**BLC-04 ferme en source.** Les UUID worker/attempt sont canoniques et non nil. stable_name doit contenir un namespace UUID simple32 canonique/non nil et le suffixe exact de worker_id, volume egal au nom +work. policy_hash et audit_hash suivent le validateur hex64 partage. Tous champs CID/inspection/create/start/emitter, observation/dates, unknown et cleanup sont absents et allocation_inconsistent=false, en plus de Reserved/rev1/quota/launch/spec/receipts deja recoupes. Ces contraintes correspondent au veritable producteur initial audit/control.rs. Aucun handler, Store, SQL ou ancienne methode AuditClient change.

Le client valide la FORME du namespace recu ; il ne connait pas le namespace central attendu et n'en prouve donc pas l'identite ni la politique de quota a partir de policy_hash. Le service/journal doivent encore recouper etat central et scope/namespace authentifies avant tout effet. Le recu retourne ne cree ni durabilite locale ni droit de start.

Verdict source actualise : BLC-01 a04 corriges a leurs gels lus ; aucun defaut bloquant residuel identifie. Les GREEN corriges/canaris, preuves finales et docs restent attendus, sans les anticiper. Service/IPC/enforcer, memoire cancel/dernier garde, D5 et production restent hors cloture de ce lot de donnees et du client HTTP.

## Audit cible de couverture du contrat - avant cloture logicielle

Lecture des cinq fonctions broker_lifecycle, deux journal_lifecycle_floor et delta du graphe broker_checkpoint. Les cinq fonctions comprennent un contrat API et un diagnostic enum ; leur nombre ne prouve pas les frontieres V3. La source corrigee reste acceptable ; les preuves ne sont pas equivalentes a cette conclusion source.

Gaps critiques constates dans l'etat lu :

| Critere canonique | Preuve actuelle | Complement minimum utile |
|---|---|---|
| Versions V3/commande, unknown nullable derriere request enum, EOF | Aucun oracle V3 dedie | Mutations d'une V3 valide via from_bytes et Deserialize publique, erreurs opaques |
| Hash commande contre vrai operation/intent du job | Seulement constructeur happy path | Commande bien formee hashee pour une autre operation/intent, avance du vrai Journal refusee et tete inchangee |
| Cap64 commandes | Non exerce |64 commandes uniques valides,65 refusees sans avance |
| Replay V3 historique et anti-downgrade des anciens replays V1/V2 | command_receipt historique seulement | advance_lifecycle ancien expected+payload apres V3 ulterieure rend meme recu ; anciens appels V1/V2 refuses |
| Grand-pere V1/V2 deja approuves | Pas exerce | Deux promotions historiques approuvees sans ApprovalSubmitted invente, premiereV3 auto-approuvee refusee depuis non-approuve |
| Dernier Cancel parmi plusieurs, grant co-ne | Graphe teste un cancel, grant ancien/frais seulement | Deuxieme cancel invalide un grant ne apres le premier ; nouvelle Export co-nee refusee |
| Admission inconnue puis vrai Reserved vers Cancelled | Test actuel s'arrete a Reserved quota tenue | Etendre le meme test avec CancelReserved pending, vrai ACK Store, quota relachee seulement apres ce fait central |

Plan de test propose a root : une seule nouvelle fonction de frontieres wire/hash/caps/chaine/grand-pere sur le harness None deja existant ; completer les fonctions existantes graphe et cancel, sans nouvelle matrice, moteur ou fonction gratuite. Les tests ajoutes sont justifies par des obligations explicites du format et les precedents defauts de deserialisation et de causalite.

Autres criteres a ne pas declarer PASS sans preuve : reopen du rejet, dual marker/accept partiel et disparition/substitution du plancher rejet ne sont pas verifies par le test de replay meme processus. Petite extension du test rejet existant possible ; sinon marquer NOT_RUN. Le canari floor actuel porte sur accepted.json, pas sur rejected.json. Aucune nouvelle matrice de crash/panne electrique n'est demandee par cette remarque.

Aucun Cargo execute par le reviewer. Les campagnes deja vertes ne ferment pas les criteres absents ci-dessus ; verdict final logiciel reserve jusqu'a preuves ou statut explicite des limites.

## Recoupement des extensions de preuve - tests d8d7c635 / 9cb7aed9

Application toujours aux gels core GEL2 et runner d559105e, empreintes recoupees. Tests lus : broker_lifecycle d8d7c6354f7e181d13180578edbd38513468a2b7a642ae384da7ba569b75fc10 ; broker_checkpoint9cb7aed956c27e51569b582c928576894e800c27aa5139b5f8caaabe0cb2573c. Log coverage-expansion/final3/broker_lifecycle-release.log :6/6 PASS RC0,1,26s ; log coverage-expansion/broker-checkpoint-final.log :6/6 PASS RC0,32,23s. Ces durees sont attribuees a leurs logs precis, pas transferees aux autres runs. Aucun Cargo par le reviewer.

Ajouts probants :

- Rejet : reopen puis observation du rejet avant retrait du marqueur (plancher vivant effectivement installe), dual marker prive0600, substitution canonique du code rejet et restauration byte-for-byte ; accept partiel protege contre reject sans suppression du temoin. Le unit test cfg(test) supplementaire ajoute des erreurs injectees apres RejectedWritten/RejectedDurable/BeforeAck et un replay exact ; il ne vaut pas panne electrique.
- Frontieres : envelope.version inconnue, unknown:null racine/public Deserialize, unknown:null sous FailureObserved, troncation et EOF ; hash forge refuse et commande transplantee vers un autre vrai job refusee ;64 commandes uniques avec prefixe preserve puis65 refusees.
- Admission/cancel : apres ACK tardif de la vraie reservation, CancelReserved apparait pending ; quota reste tenue avant mutation centrale, puis vrai Store Cancelled/quota=false et ACK du recu central. Pas de terminal runtime ou usage invente.
- Graphe : Export co-ne avec deuxieme cancel refuse ; grant ne apres premier cancel mais avant deuxieme refuse ; nouveau grant strictement apres dernier cancel accepte jusqu'au helper claim. Vrais Store/Journal, faits runtime toujours synthetiques.

Oracles encore non discriminants dans l'etat d8d7c635, signales a root et Luna avant cloture :

1. Le V2 initial est ecrit depuis accepted.head mais le test anti-replay rappelle advance_supervised depuis rev1, qui est deja la sortie V2. Sans garde downgrade cet appel serait aussi Conflict par hash different : il ne prouve pas le refus d'un ancien replay exact. Aucun checkpoint V1 n'a ete avance avant V2, donc l'appel V1 n'est pas non plus un ancien replay exact. Inserer une vraie avance V1 puis rappeler les expected exacts historiques de V1/V2 apres V3.
2. Le bloc grand-pere fait seulement V1 approuvee vers V2 approuvee puis V3 ; la branche directe V1 approuvee vers V3 n'est pas parcourue. Boucle via_v2 false/true et controle negatif premiereV3 auto-approuvee sans commande anterieure sont proposes.
3. Le replay V3 compare uniquement le numero de revision, pas le hash du recu original ; comparer head integralement.
4. Deux mutations tres ciblees restent utiles dans la meme fonction : version de commande inconnue et unknown:null sur CancelRequested{} via wrapper et Deserialize publique. La mutation actuelle vise FailureObserved et pas la variante vide choisie pour fermer Serde.

Ce sont des manques de preuve, pas de nouveaux defauts applicatifs. Une seule fonction de frontieres reste suffisante ; aucune nouvelle matrice ou moteur demande. La cloture des criteres correspondants demeure reservee a leur preuve correcte.

## Oracles de frontieres corriges - source a3716314

Relu dans le checkout : tests/broker_lifecycle.rs SHA `a371631492665e8f593b1873f3136f3adac1dfad8fc1d5d61154c593625898c4`. Cette version remplace les oracles d8d7c635 et b1d783d0 ; aucun GREEN ancien n'est transfere.

Les reserves d'oracle sont corrigees dans la meme fonction : vraie avance V1 avant V2 et reutilisation des expected exacts historiques apres V3 ; recu replay V3 compare operation/revision/hash ; promotions approuvees directes V1 vers V3 et via V2 dans deux harness separes ; refus de premiereV3 auto-approuvee qui contient bien Decision Approved pending mais aucune ApprovalSubmitted anterieure ; commande NOUVELLE hashee avec mauvais intent dans la meme operation, prefixe immutable conserve ; version commande2 refusee ; unknown:null CancelRequested refuse par from_bytes et Deserialize publique de l'enveloppe. Les tests sont donc discriminants pour les criteres vises. Aucun nouveau defaut applicatif ou d'oracle identifie a cette lecture.

La cloture de couverture reste conditionnee au GREEN de ce SHA final incluant le dernier negatif auto-approval, et a son recoupement de provenance ; les6PASS des versions precedentes ne suffisent pas. Aucune nouvelle matrice ni nouveau Cargo par le reviewer.

## Verdict final logiciel et couverture ciblee

**CLOSE au perimetre des donnees lifecycle V3, lecteurs Journal et raccord HTTP d'admission.** BLC-01 a04 reproduits puis corriges ; oracles discriminants relus, aucun defaut bloquant source ou couverture critique residuel identifie dans ce perimetre. Aucun Cargo, build, moteur ou source de produit execute/modifie par le reviewer. Les SHA app GEL2 et runner d559105e demeurent inchanges.

GREEN final recoupe : tests/broker_lifecycle.rs a371631492665e8f593b1873f3136f3adac1dfad8fc1d5d61154c593625898c4, copie reviewer-final/final et manifeste identiques au checkout ; log reviewer-final/broker-lifecycle-release.log6/6 PASS,0ignored/0filtered,1,41s,RC0. Fmt et Clippy strict reviewer-final PASS RC0 sur ces empreintes. tests/broker_checkpoint.rs9cb7aed956c27e51569b582c928576894e800c27aa5139b5f8caaabe0cb2573c, log coverage-expansion/broker-checkpoint-final.log6/6 PASS32,23s ; pas de rejeu injustifie apres le seul ajout du negatif lifecycle. Rapport Luna relu, SHA c634cb655d7c2bcf72ca55e5c971ccbaba1a17cdd9afc1e0a6b45424ae0e8b44, dont la derniere section designe le gel final et remplace les preuves initiales moins discriminantes.

Autres logs complets relus : journal_lifecycle_floor2/2 PASS ; broker_admission_client4/4 PASS (vrais Store/Gateway loopback et middleware technique de mutation) ; broker_journal12/12 PASS ; unit Journal5 parents PASS,2 enfants ignores par defaut invoques par leurs parents, non additionnes. Le unit rejection IO fait partie des cinq, pas trois tests supplementaires pour ses trois stages. Les fonctions de contrat API et diagnostics restent incluses dans ces comptes, pas des tests d'isolation runtime. Build release PASS conserve du gel app ; derniers changements uniquement de tests ne sont pas presentes comme changement de binaire produit.

Les criteres autrefois reserves sont maintenant prouves aux frontieres ciblees : formats V3/commande et null inconnu dans CancelRequested, EOF/troncature, contexte operation ET intent d'une nouvelle commande,64/65 et preservation du prefixe, recu V3 historique operation/revision/hash, vrais anciens replays V1/V2 refuses apres V3, promotion directe/viaV2 approuvee et refus d'auto-approval avec Approved pending, Review/commande en deux commits, rejet reopen/partials/dual/plancher, deux cancels/grants co-nes et frais, Admission inconnue conservee puis CancelReserved central reel. Les commandes locales ne rendent pas un RPC non applique : reponse perdue reste un fait inconnu.

Precision pour les documents : la quota CENTRALE est liberee par la transaction CancelReserved du Store, avant l'ACK du checkpoint local. Le test verifie bien quota tenue avant cette transaction puis0 apres ; une reponse perdue peut laisser local pending avec quota centrale deja liberee. Ne pas attribuer cet effet au cancel local ni a son recu. Le client valide la forme du namespace et des noms, le service doit toujours recouper leur identite centrale authentifiee.

L'incident de checkout rapporte par root correspond a une observation du mauvais checkout ; root a controle source et reflog sans reset ni restoration. Le reviewer a recoupe les empreintes app intactes, sans restaurer de fichier ou refaire un historique Git. Ce point n'est pas un echec produit ou un test supplentaire. Les erreurs de fixtures/tests restent conservees dans leurs logs sans relachement des gardes produit.

Limites inchangées : hors qualification service/IPC/enforcer, cancel memoire/dernier garde, start RPC retarde D5, Preparing sans CID et reconciliation operateur, mort CLI, runtime/horloge reels, Docker/Podman, OS/cloud/PKI, E2E global, charge100 et production. Les faits runtime/horloge du graphe sont synthetiques, les Store/reçus/Journal et HTTP loopback sont reels. G09/QA/schema2/billable=false ne changent pas par ce lot. La revue documentaire et les artefacts qualifies prepares par root seront recoupes apres leur gel ; aucun SHA de preuve ou de binaire final n'est encore anticipe ici.
