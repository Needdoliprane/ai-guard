# Pré-revue — visibilité de FailureObserved après H10

Lecture seule du contrat `docs/engineering/IMPLEMENTATION_PLAN.md:89–100`. Aucun code WIP évalué comme livré, aucune source/doc modifiée, aucun Cargo, moteur, QA, probe, service, secret ou recherche supplémentaire. Cette note prépare la revue du gel GREEN ; elle n'approuve pas une implémentation encore absente.

## Conception

**Aucun P0/P1 intrinsèque identifié dans ce contrat.** Pour chaque appel effectif d'`Execution.failure`, tenter un rapport fermé même si la classification spécialisée ne sait pas préciser l'erreur ferme la lacune de H10. Le fallback `broker_step=failure_observed` affirme seulement l'observation d'un échec dans la phase déjà établie côté contrôle. `runtime_effect=unknown`, absence de texte/causes supposées et usage de l'outbox existante empêchent d'en déduire absence d'effet, permission, terminal, fencing ou libération de quota.

Les contextes ownership_commit, ownership_reload, created_inspection et created_exchange correspondent à la fenêtre bornée par owned29/Failure30 avant Created. Ils peuvent identifier une frontière d'erreur, pas le retour d'un ACK ou le succès du moteur. Le renommage privé PreclaimStep→BrokerStep doit conserver exactement les labels sérialisés historiques. Les nouvelles variantes restent fermées ; le lecteur garde les labels legacy et huit champs sélectionnés.

## Critères précis à confirmer au gel

- `failure_report` conserve tout diagnostic spécialisé déjà disponible ; le fallback couvre l'absence de provenance connue sans effacer Native/Runtime/Broker ni reprendre Display/Debug/chaîne d'erreur. Phase et effet viennent d'enums contrôlés, jamais d'un texte ou d'une entrée worker.
- La régression utile appelle **Execution.failure**, avec vrai Gateway/Store/Journal/Writer et erreur privée inconnue. Elle exige FailureObserved local puis événement réellement reçu et état d'outbox/ACK cohérents au même scope/opération. Le canari doit être absent des bytes persistés et livrés, sans compter seulement sur le masquage d'un nom de clé. Un test pur du fallback ne prouverait pas le raccordement qui manquait dans H10.
- Les erreurs spécialisées gardent leurs champs, sens et labels ; les erreurs inconnues restent inconnues. Le nouveau parser accepte exactement les nouveaux labels fermés et refuse toujours identités/seq/phase/effet/provenance invalides, sans exposer la valeur privée dans ses erreurs. Ni origine libre ni neuvième champ arbitraire ajouté.
- Les contextes entourent les appels déjà présents sans déplacement de garde, lecture, horloge, await, mutation/CAS ou barrière. L'échange Created signifie frontière extérieure ; il ne prouve ni envoi ni ACK. Aucun nouveau cache, retry de start, claim, Arm ou durée.
- Publication et ACK réutilisent le chemin existant : masquage avant persistance, enqueue durable avant HTTP, ACK exact ; après perte de réponse, pas d'événement déclaré absent ou acquitté à tort. Une erreur Full/binding/audit/file pleine peut encore empêcher la livraison ; ne pas affaiblir ses refus pour garantir l'affichage. L'échec de livraison reste opaque et ne transforme pas l'action en succès.

Deux limites de rédaction à conserver : « aucune IO/await supplémentaire » vise le callback et le chemin de succès ; publier après une erreur jusque-là non classée ajoute intentionnellement le travail de rapport sur le chemin d'échec. « Tout échec » désigne ceux réellement observés par Execution.failure : cette seule couture ne couvre pas une ouverture échouée avant sa création, un panic/abort, un processus tué ou un timeout global. Une garantie de visibilité universelle serait excessive.

## État de preuve

RED/GREEN et implémentation : **en cours, non revus à cette pré-revue**. Aucun PASS anticipé. H10 demeure FAIL d'observation avec couverture diagnostique manquante ; sa cause précise et l'intervention cache concomitante restent distinctes. Ce lot de visibilité ne qualifie ni startup, ni stop autonome, ni reprise/D5 et ne ferme aucun gate. Aucun nouvel essai runtime demandé ou exécuté par cette note.

Prochaine étape de revue : lire uniquement le delta figé GREEN, recouper tests réels et checks Luna, puis annexer les SHA, résultats et limites ici. Aucun autre travail avant ce gel.

## Annexe — revue du code figé AprèsH10, avant résultats S4

Base HEAD `6c7b63ebcef4a92cadfc2ec95626033f48011576`, delta des sept fichiers ci-dessous. Lecture seule du diff et des appels directement concernés ; revue parser recoupée indépendamment. Aucun Cargo, moteur, QA, secret ou recherche externe. `git diff --check` RC0 au moment de cette lecture. Les autres éditions documentaires de Root sont hors revue.

**Aucun P0/P1 reproductible identifié dans ce delta.** Cette conclusion porte sur le raccordement et les frontières de données ; les checks et le build S4 restent en attente.

- `executor/diagnostics.rs:54–88` : `classify(Some)` est retourné intégralement par `failure_report`, avec enforcer/runtime/broker connus conservés. Sinon, le seul ajout est le label fermé `failure_observed`, phase passée comme enum et effet `unknown`. Aucun Display/Debug/texte de chaîne n'entre dans le DTO Serialize ; aucun nouveau champ libre, type d'autorité ou DTO de protocole. Les anciens labels sérialisés sont inchangés par le renommage privé BrokerStep.
- `executor/journal.rs:162–191` : même append FailureObserved en premier, son résultat ignoré comme auparavant ; puis tentative de `fact_event` pour tout appel qui atteint ce point. Le commentaire exclut explicitement une preuve de publication de l'append. Un échec du rapport conserve le message fixe opaque et ne déclenche ni récursion, ni retry de start, ni release de quota. `results.rs:277–301`, inchangé, relit strictement puis masque/valide avant enqueue durable, POST des mêmes données et ACK UUID/seq exact. Une erreur de Full, binding, queue, HTTP ou ACK peut encore empêcher la livraison ou laisser du pending ; aucune livraison universelle n'est annoncée.
- `executor.rs:692–734` : les quatre contextes annotent le patch ownership, sa relecture, la construction de l'inspection et l'échange Created déjà présents. Même ordre, mêmes await et mêmes appels, aucune lecture/horloge/garde/permission/mutation supplémentaire sur le chemin de succès. CreatedExchange couvre aussi un refus local de la mutation avant HTTP : il n'atteste donc ni POST ni ACK. Les erreurs de validation de draft hors de ces annotations restent couvertes par le fallback sans fausse cause précise. `helpers.rs` et `journal.rs:261` ne changent que le nom du type des contextes existants.
- `journal_tests.rs:481–582` raccorde réellement Execution.failure à Gateway/Axum, SQLite, Journal et Writer. Il vérifie d'abord FailureObserved, puis l'unique événement V4 et le même événement public, UUID et audit_seq ACK positif exact ; absence du canari dans le checkpoint et la réponse publique. Le Runtime pointe le binaire de test mais aucune méthode moteur n'est appelée par cette couture. Le pair IPC existant reste vide, puis est fermé et attendu avant les assertions ; le test exige zéro slot, zéro opération/helper et aucun managed worker. Cela prouve la publication dans cette fixture saine, pas la récupération après panne de livraison.
- `tests/support/broker_runtime.rs:413–417` ajoute seulement les cinq labels prescrits ; huit champs sélectionnés, contrôles d'identité/phase/source/origine, bornes600/5 et polling inchangés. `tests/broker_runtime.rs:73–99` exige Some pour chaque label, effet unknown, étapes native/runtime nulles, huit champs et exclusion du canari libre. Il s'agit d'une compatibilité de lecteur, pas d'une preuve runtime.

### RED constatés, GREEN en attente

Logs lus dans `/private/tmp/ai-guard-s3-red-20261031-225412` : les deux cibles compilent puis sortent RC101. `01-unknown-failure-debug.log` échoue sur l'absence du checkpoint V4 après les assertions de FailureObserved ; aucun échec de compilation ou de précondition ne remplace l'oracle. `02-parser-release.log` échoue sur `invalid diagnostic stage label` au premier nouveau label. Les assertions du journal sont conservées dans le delta gelé. Ces RED reproduisent les deux lacunes visées ; ils ne prouvent aucun GREEN.

S4 annoncé : trois tests diagnostics, un raccordement réel et quatre parseurs, puis fmt/Clippy strict/build. Résultats non terminaux à cette annexe : **en attente**, aucun PASS anticipé et aucun nouveau test lancé par le relecteur.

### SHA256 des sources relues

| Fichier | SHA256 |
|---|---|
| src/runner/broker/executor.rs | e20d2bf0853f33afa2de4d4b669af03f843ab027a956942ef30d8b8e687c25dd |
| src/runner/broker/executor/diagnostics.rs | 286e79f5d92ccc00f1223e7fa86e17a50e59596e0b9f2a9efc9f873bc3f76346 |
| src/runner/broker/executor/journal.rs | 134c3aa3ff263608bea06f0f502c005d1f59b16a2f3ba9fc592c5566b83c352e |
| src/runner/broker/executor/journal_tests.rs | 7e615ac2aa4adceb927ce1dc017c21f16a3cc6966f505ef686886b348b2708fc |
| src/runner/broker/executor/helpers.rs | 97b38266260541bf710a6cc735b9b07e8fadeb542c07fde07765527cf1a3379d |
| tests/broker_runtime.rs | 3bdbe1ef16fc1f157a837fe78c4eca3690ce7f4877ab5d9d02e02d1afcc17129 |
| tests/support/broker_runtime.rs | 91792c629f65eeadc074bd7fa77b9d31a9c4d574b70eb8896a815c2991fb4873 |

La couverture reste exactement celle d'Execution.failure ; ouverture échouée, panic/abort, processus tué et timeout global ne deviennent pas visibles par cette seule modification. H10 reste FAIL d'observation, sa cause exacte demeure inconnue. Aucun runtime de ce nouveau code exécuté dans cette revue, aucun gate G09/D5 fermé et aucune qualification production déduite.

## Annexe finale BACKEND — S4 terminal

Logs et RC réellement lus dans `/private/tmp/ai-guard-s4-20261031-230349` ; ils remplacent l'état « en attente » de l'annexe précédente. Aucun test exécuté ou relancé par le relecteur.

| Preuve Luna | Résultat constaté |
|---|---|
| 01-executor-diagnostics | RC0, 3 fonctions PASS, 0 échec/ignoré |
| 02-unknown-failure-debug | RC0, 1 fonction PASS, 82,31s, 0 échec/ignoré |
| 03-parser-release | RC0, 4 fonctions PASS, 0 échec/ignoré |
| 04-fmt-check | RC0, log vide |
| 05-clippy | RC0, 1m31, contrôle strict all-targets selon la commande Luna du lot |
| 06-build-release | RC0, 0,95s, build release réutilisant les artefacts déjà construits |

**8 fonctions uniques PASS**, sans additionner les deux exécutions RED S3. La cible réelle passe après l'échec initial au checkpoint V4 absent ; le parseur passe après le refus initial du nouveau label. Le résultat du raccordement confirme dans la fixture saine la réception publique du rapport opaque et l'ACK exact de son événement durable, avec zéro autorité runtime acquise. Il ne mesure ni le délai de visibilité en session réelle ni une résistance à toutes les pannes de livraison.

Le diff pré/post-format capturé par Luna est limité à diagnostics.rs et journal_tests.rs : retours à la ligne autour d'un tuple enum/label et d'une assertion, sans changement de valeur, condition, message ou oracle. Les sept SHA de sources relues dans le tableau précédent correspondent toujours au checkout final S4, vérifiés par lecture ; aucune nouvelle modification applicative à réévaluer pour ce formatage.

Traçabilité recoupée : le manifeste final contient159sources ; les rapports indépendants de Root `/private/tmp/ai-guard-s4-root-verify.rVf11T/{checkout-check,extracted-check}.txt` comportent chacun159lignes OK. `binaries-check.txt` confirme les deux copies. Les deux binaires archivés sont de mode600, et leurs SHA ont été relus directement.

- Manifeste SHA256SUMS : `cc08455585a3c35978cff25c820b27ceda0093fb785f381a512c696a30771fd8`.
- Archive sources : `9569f60ace38e3f054658657c9a4d42ad165fcce3c663318e7091452e1a588f3`.
- Copie agentsandbox : `2ffdec56820e9ea4d3f528274f414c3a465c6284a25c1bb6ffac5a4935ee5f71`.
- Copie ai-guard-provider : `222e7e07ba94d2bc3e505fa839fb9bdbd808e24776c5b1d127500b709493c8a5`.

**Conclusion indépendante backend : aucun P0/P1 restant identifié dans les sept fichiers et le contrat revus ; les deux régressions ont un RED réel puis un GREEN ciblé, et les checks S4 sont terminaux RC0.** Le repli conserve les diagnostics spécialisés, n'invente ni cause ni effet, et s'insère dans l'outbox existante sans modifier le parcours de succès ou ses autorisations. La tentative de rapport ne garantit toujours ni l'append local réussi ni sa livraison lorsque Full/binding/queue/HTTP/ACK échoue.

Émission après un nouvel échec moteur réel avec ce code : **NOT_RUN**. H10 et son défaut de visibilité historique restent FAIL, cause précise non établie ; aucun nouveau runtime, aucun test fournisseur ni qualification visuelle/frontend dans cette annexe. G09/D5 restent ouverts. Le complément frontend en cours est volontairement hors de ce gel backend et attend une revue distincte après gel.
