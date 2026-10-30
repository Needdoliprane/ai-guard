# Revue ciblée de conception — checkpoint broker V2

Revue du 30 octobre 2026, worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, HEAD vérifié `3e2cffa86c0d0c4784507e78f289419a9e39890f`, état Git propre au passage. Document relu : `/tmp/ai-guard-broker-checkpoint-v2-design.md`, SHA-256 `089df560a60cff22b479aabeaa82ae907988134250fca3dcb481f1cd1405311d`. AGENTS/RTK et contrats du périmètre déjà lus. Revue de conception et de sources uniquement ; aucun test, build, moteur, modèle ou runtime exécuté, aucun fichier du dépôt modifié.

## Verdict

Le découpage proposé est constructible sans outbox générique ni nouveau stockage annexe. Les vrais DTO couvrent les requêtes et résultats annoncés. Le pending admission avant managed/Worker est nécessaire et représentable ; aucun Worker fictif n'est requis. Les constructeurs publics typés proposés par root rendent les nouveaux wrappers utilisables sans JSON artisanal. CAS, replay historique V2, domaine de hash distinct, anti-downgrade après promotion et plancher vivant peuvent réutiliser le journal actuel.

**Contrat à compléter avant code : un défaut concret de frontière V1/V2 et deux préconditions de validation à rendre explicites.** Ce sont des écarts du contrat proposé, pas des vulnérabilités produit reproduites. Aucune donnée persistée ne constitue un Grant ou une preuve d'authenticité du contrôle.

## D01 — promotion : inspecter tout le préfixe V1, pas seulement la tête

Document ligne 104 : « journal encore sans managed legacy ». Le critère doit explicitement signifier **aucune révision V1 confirmée n'a contenu managed**.

Condition concrète : une chaîne V1 contient rev1 avec managed, puis rev2 avec managed=None. L'API V1 actuelle l'autorise : `journal.rs:503` valide seulement le contexte candidat, sans comparaison avec le payload précédent ; `runner.rs:551-552` accepte managed=None. Le loader V1 ne vérifie pas non plus de monotonie de ce champ. Si la promotion n'examine que la tête, ce journal entre en V2 comme une session neuve malgré son historique géré, en contradiction avec l'interdiction d'adoption implicite.

Correction minimale : conserver un fait interne calculé pendant le chargement de toute la chaîne, ou examiner le préfixe au premier V2 ; refuser la promotion si un managed y a jamais été présent. Ne pas modifier rétroactivement la lecture ou les bytes V1 pour corriger cette frontière. Régression ciblée à prévoir par Luna : V1 initial → managed → None → tentative V2 refusée. **Bloquant tant que ce sens n'est pas retenu.**

## D02 — autorité runtime : la donnée nécessaire doit exister au seuil owned/Arm

Document lignes 91/94/98 : admission pending avec managed=None est valide, mais le contrôle complet des owned/Arm est annoncé « contre RuntimeAuthority disponible ». L'envelope V2 ne stocke aucune RuntimeAuthority ; sa seule source actuelle est `runner.managed.binding: Option<RuntimeAuthority>` (`runner/managed.rs:18`). Le handle conserve seulement son fingerprint (`runtime/managed.rs:111`), lequel dépend notamment de l'endpoint, engine_id et du binding Podman/invocation (`runtime/managed.rs:44-63`). Ces composants ne sont pas déductibles de LaunchSpec ou d'Arm.inspection.

Condition : admission acceptée et opérations ultérieures présentes, mais runner.managed=None ou binding=None. Comparer deux chaînes de fingerprints fournies ne permet pas de recomputer le lien avec l'autorité complète annoncé dans le contrat.

Choix minimal : admission pending/acceptée peut rester sans managed ; **le premier owned ou Arm exige ensuite un managed réellement préparé et binding=Some**, que la validation pure reçoit par getter étroit. Autre choix possible : conserver une autorité typée séparée dans V2, au prix d'un champ supplémentaire. Ne pas fabriquer ManagedCheckpoint pour passer l'admission. Un record incomplet reste sans effet autorisant ; cette règle concerne la cohérence des données, pas leur authenticité. **Précondition à trancher avant implémentation, sans quoi la validation annoncée est conditionnelle et incomplète.**

## D03 — durabilité avant RPC/create : préciser les transitions de naissance

Le document impose l'ordre durable avant RPC/create (lignes 78/82), mais ses règles de transition (ligne 106) ne disent pas explicitement si une opération nouvelle peut apparaître directement avec response=Some, ni si un helper neuf peut naître avec owned=Some. None→Some seul ne définit pas la valeur initiale.

Pour que la chaîne apporte une preuve locale de la préparation antérieure, préciser : nouvelle opération avec response=None ; réponse Some seulement pour une requête déjà confirmée dans la révision précédente ; helper neuf owned=None, puis owned=Some uniquement après une préparation confirmée. Même garde à appliquer au premier V2, ou exception de promotion décrite précisément. Le loader doit revalider ces règles entre révisions, pas seulement advance_supervised. Une politique/permission/helper_id déjà préparés ne peuvent être remplacés pour contourner cette séquence.

Cela n'établit pas qu'un RPC ou create réel a respecté l'ordre : ce sera la responsabilité du caller et de ses tests. Cela évite seulement qu'un enregistrement ajouté en un seul ACK soit présenté comme deux étapes durables antérieures. **Précision nécessaire si cet ordre est une propriété vérifiée du journal ; sinon le document doit le réserver explicitement au caller.**

## Liens causaux à expliciter, déjà signalés par root

- Decision : unicité `(request_id, phase/human_approved)`, pas request_hash seul. `policy.rs:36-42` neutralise approval dans ce hash. Initial et approved doivent coexister ; leurs requêtes historiques sont comparées à l'intention normalisée, pas toutes au current_request approuvé ultérieur. Receipt.version, phase, request_id/hash et audit doivent être contrôlés explicitement ; DecisionReceiptV1 n'a pas de méthode validate autonome.
- Arm.parent : égalité intégrale avec un Worker contenu dans une réponse antérieure acquittée du même job, parmi Admission/WorkerMutation/WorkerClaim. `ArmV1::validate_for_parent` (`control/supervision.rs:492+`) ne prouve que forme/identité/révision/état ; un Worker libre de même identité ne devient pas un snapshot central lié. Conserver ce parent historique ensuite, même si le worker courant avance.
- Helpers : la liste helpers séparée ne donne aucun ordre causal à permission. Au point de l'Arm, résoudre sa permission et son owned vers les éléments préparés, et chaque receipt vers une opération antérieure acquittée. ImportVerification exige l'import terminal réussi de ce graphe ; Export exige l'ExportRequest correspondant. La dérivation `helper_execution_policy_v1` ne nécessite pas un parent nouvellement inventé : admission/attempt/launch déjà liés suffisent aux champs immuables de la policy. Aucun cycle nouveau n'est nécessaire.

## Points confirmés et pièges d'implémentation

- Les snapshots historiques ne doivent pas être validés contre le worker courant ni l'horloge actuelle. Le clock_check persisté se valide contre l'Arm/boot/deadline persistés ; la fraîcheur d'exécution relève du futur service.
- Admission refusée garde accepted=false/refusal/worker=None. Receipts ne se valident pas tous avec le même domaine : supervision admission utilise le wrapper SupervisedAdmissionV1, WorkerClaim utilise ArmedStartClaimV1, mutations ordinaires WorkerMutationV1 (`audit/supervision.rs:163-174`, `Input::hash`). Les receipts helper/export n'ont normalement pas de revision worker ; ne pas l'inventer ni l'exiger uniformément.
- Les DTO résultats/Worker/Decision n'ont pas tous un validate existant. Le nouveau validateur pur doit recouper leurs champs et bornes ; Decision n'a même pas deny_unknown_fields (`model.rs:19-25`). La détection serde_ignored sur toute l'envelope, EOF et erreurs opaques est donc nécessaire, même avec des wrappers fermés. Aucun remaniement du DTO historique requis.
- Les règles de replay décrites sont cohérentes : comparer un replay V2 historique exact avant sa transition contre la tête ; l'ancien advance V1 refuse au contraire tout replay dès tête V2, choix délibéré root. Le loader mixte doit contrôler version/digest et transitions à chaque révision, conserver rev0 V1 exacte et refuser V2→V1. Le plancher partagé ne promet toujours aucun anti-restore après restart.
- Les caps proposés peuvent être vérifiés avant append puis avant sérialisation/publication ; le plafond 32 MiB reste global. Ils ne sont ni quota physique ni qualification 100 workers. Debug opaque doit également couvrir l'enum public d'opérations, pas seulement les structs enveloppes.

Aucune refonte structurelle supplémentaire requise. Le verdict d'implémentation/tests et toute qualification runtime restent **NOT_RUN** dans cette revue de conception. Note privée 0600 ; aucun changement repo autorisé ou effectué.

## Clôture après décisions root

Root retient explicitement les corrections suivantes, obligatoires pour le contrat de code :

1. Promotion interdite dès qu'une révision de tout le préfixe V1 contient managed ; pas seulement la tête. **D01 CLOSED au niveau conception.**
2. Decision identifiée par `(request_id, phase)` ; chaque requête historique comparée à l'intention normalisée. Arm.parent égal intégralement à un Worker d'une réponse antérieure acquittée, et non à un Worker libre de même identité.
3. Nouvelle opération exclusivement response=None ; Some seulement si la requête exacte existait dans la révision précédente. Nouveau helper owned=None ; Some seulement dans une révision suivante. Ces règles couvrent aussi le premier V2 et la relecture de toute chaîne. **D03 CLOSED au niveau conception.**
4. Tout helper préparé, tout Arm ou owned exige `runner.managed.binding=Some`. Admission pending ou acquittée peut rester managed=None avant préparation. Aucune seconde autorité dupliquée dans V2 ; vue pure cratée par références sur les vrais champs privés, sans publication de ManagedCheckpoint ni réinterprétation JSON. **D02 CLOSED au niveau conception.**

**Verdict final de conception : CLOSE sous intégration de ces décisions dans le contrat actif.** Aucun défaut bloquant supplémentaire identifié à cette lecture ciblée. Les points D01/D02/D03 du document initial restent conservés ci-dessus pour leur condition et leur raison ; ils ne constituent pas des constats ouverts contre le contrat corrigé décidé par root. Le SHA en tête désigne le premier document effectivement relu, pas un plan corrigé prétendument rehashé. Toute application effective au code/loader/API et toute régression restent à vérifier après implémentation ; aucun test exécuté par cette revue.
