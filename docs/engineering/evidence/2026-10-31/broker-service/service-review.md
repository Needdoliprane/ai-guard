# Revue indépendante ciblée service/CLI — 31 octobre 2026

Revue en lecture seule du worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, base c4c4d2a modifiée. Aucun fichier source édité par le reviewer ; aucun Cargo, test, moteur, service ou fournisseur exécuté. AGENTS, contrat d'architecture et règles de code relus ; skill security-audit utilisé en guidance pour cette revue ciblée. Le rapport demandé par l'orchestrateur est le seul artefact écrit. Le skill projet security-review absent n'est pas revendiqué.

Périmètre : `src/broker/service/{profile,protocol,client,mod,server,jobs,targets}.rs`, `src/broker_cli.rs`, raccords de `src/main.rs`, `AuditClient::supervision_identity` dans `src/runner.rs`. Lecture supplémentaire seulement pour établir les coutures : getter `retained_registrations`, admission batch, événements/exports et validations des checkpoints. Le module enforcer écrit précédemment par ce reviewer n'est pas approuvé indépendamment ici ; le nouveau port admission écrit par Root a été relu comme interaction ciblée.

Les gravités ci-dessous qualifient l'effet établi par le code ; elles ne prétendent pas à un effet moteur reproduit. **Exécution/runtime : NOT_RUN pour tous les constats.** Pas de conclusion de sécurité exhaustive ni de qualification production.

## R1 — moyenne, blocage comportemental : un ancien Cancel arrête un export postérieur à ce Cancel après restart

**Confirmé par le chemin source ; ouvert lors de la première lecture.** `src/broker/service/server.rs:1189` réduit les commandes durables à `any CancelRequested`, invalide le compteur mémoire, puis `:1206–1217` restaure chaque cible et appelle `slot.cancel(...)` pour toutes si ce booléen est vrai. Le getter `src/runner/broker/executor.rs:271` extrait toutes les Arms ACK sans filtre causal, donc aussi un helper Export dont le grant est né après le dernier Cancel.

Scénario admissible par le contrat V3 : worker terminal, CancelRequested C confirmé, ExportRequest E strictement postérieur à C puis ACK, helper Export armé/démarré sous ce grant ; broker redémarre pendant cet export encore dans sa deadline. La reprise transforme C en annulation de E. La cible est exacte et du bon tenant, mais le motif temporel est faux : une demande d'arrêt historique ne couvre pas ce nouveau grant. L'export peut être interrompu alors qu'aucune nouvelle annulation n'a été demandée. Il ne s'agit pas d'un bypass d'authentification ou d'une fuite inter-tenant.

Invariant correct : conserver l'arrêt à la deadline originale pour chaque cible ; appliquer l'annulation historique seulement aux cibles couvertes par le dernier Cancel durable. Pour Export, retrouver l'ExportRequest exact lié à HelperPermission::Export et sa naissance relative à la dernière révision Cancel. Un NOUVEAU Cancel couvre ensuite toutes les cibles antérieures. Le compteur mémoire observé peut être seedé au max pour tous les slots ; sa valeur ne constitue pas une preuve durable de causalité et ce seed n'est pas lui-même un Cancel.

Test négatif requis, avec contrôle positif : historique réel C→E/ACK→ExportArm/claim, restart ; le worker/anciens helpers reçoivent le stop historique, le helper Export post-C reste non annulé avant deadline. Ajouter un nouveau Cancel après E doit bien l'arrêter. Ne pas compter un `Created`, une absence ou un timeout comme terminal/fencing.

## R2 — moyenne, admission : un helper arrêté ou en reprise n'empêchait pas une nouvelle réservation centrale

**Confirmé dans les sources initiales ; corrigé en source par Root et delta relu, tests encore NOT_RUN.** Initialement `server.rs:612–619` ne contrôlait que les bits statiques fenced/stopping ; l'enforcer n'était contrôlé qu'au startup. Après panne du child pendant que le broker vivait, un nouveau Submit/Approve/Resume pouvait aller jusqu'à Decision, copie et POST Admission centrale (`executor.rs` initialement :458–483) avant le premier register enforcer. Celui-ci refusait ensuite le start. Pas de nouveau start sans garde, mais une nouvelle réservation/quota et des préparations inutiles alors que la reprise stop-only n'était pas complète, contrairement à l'obligation pré-admission.

Delta Root relu : `src/broker/enforcer/client.rs:352` résout le client actif, check_health avant/après un vrai RPC Status, vérifie epoch/healthy/capacité/count et toutes registrations ACK. check_health couvre inconnus initiaux/perdus, HUP noyau et progression. `server.rs:612` est async et les entrées/reconciler l'attendent. Le batch vérifie avant décision/préparation et avant publication d'Admission ; `executor/journal.rs:235–241` vérifie immédiatement avant tout POST Admission, y compris un pending de résultat inconnu. Un ACK déjà stocké est rendu en `rpc_checked:162–164` avant cet appel.

Cette distinction est nécessaire : rejouer un POST idempotent inconnu peut créer sa première réservation si le premier POST n'a jamais atteint Store ; ce n'est pas une lecture du résultat. En l'absence de lookup read-only officiel, garder le pending jusqu'à la readiness restaurée.

Tests requis : après startup sain, helper SIGSTOP/absence/perte santé/slots inconnus ou ACK incomplet ; nouveau Submit ne doit produire ni Admission centrale ni réservation/quota. Contrôle positif après vraie restauration complète, et ACK déjà stocké consultable sans nouveau POST. Test de panne juste après préparation, avant Admission. La course inévitable entre santé échantillonnée et effet suivant ne se transforme pas en atomicité OS.

## R3 — moyenne, identité de commande : Export→Resume/Cancel changeait de type sous le même UUID

**Confirmé en source ; identité commune confirmée par le mainteneur de ce lot ; correction Root en cours.** Les branches Approve/Cancel/Export et `reject_event_command_reuse` (`server.rs:801`, `:908`, `:1040`, `:1471`) consultaient commandes V3 et événements V4. Resume vérifiait ces commandes puis `resume_event`. Elles ne consultaient pas les ExportRequest de supervision V2.

Après ExportRequest de UUID X durable, un Resume avec command_id X peut ajouter `broker.resume.requested` et recevoir un ACK ; Cancel X peut devenir une nouvelle annulation. Le sens inverse est déjà refusé lorsque X est un événement/commande. Le contrôle central ne ferme pas cette asymétrie : `audit/supervision.rs:966` crée un UUID d'événement distinct pour Export, tandis que X est son operation_id de mutation. Les contrats V3/V4 valident l'unicité dans leurs propres listes, sans unicité inter-listes. C'est un défaut d'idempotence et de causalité de commandes d'un même job, pas un gain de rôle ou un accès au tenant B.

Autre fenêtre du même invariant : `targets.rs:112–159` publie durablement `export-inputs/<profile>/<UUID>.json` avant la naissance d'ExportRequest. Après crash entre ces deux opérations, cet input seul réserve déjà X à un export de contenu immuable. Il doit empêcher qu'un autre type IPC réutilise X, même sans op Export présente au checkpoint. Root a confirmé devoir couvrir aussi ce préfixe.

Tests négatifs requis : Export input seul, puis ExportRequest pending, puis ExportRequest ACK, puis export terminé ; pour chaque préfixe réutiliser X via Resume/Cancel/Approve doit Conflict sans événement/commande divergente ni nouvel effet. Le replay Export exact doit rendre le même contenu ; autre destination/job sous X doit rester Conflict. Un refus Cancel peut avoir invalidé les gardes mémoire avant l'await (fail-closed), mais ne doit pas fabriquer un ACK durable d'un autre type.

## Point de placement à recouper après correction R2

`server.rs:798` vérifie maintenant l'enforcer avant le replay pur d'une ApprovalSubmitted déjà ACK (`:802–827`). Un helper indisponible/plein peut donc empêcher de relire ce reçu en répétant exactement la commande, alors que Status/List restent accessibles. Si la promesse « ACK existant consultable » couvre le replay de commande, déplacer la gate après la branche de replay pur. Même question pour le retour d'un export déjà terminé. Aucun bypass n'est déduit ; le raccord doit définir une réponse de rejeu qui n'émet aucun nouvel effet. Root a reçu ce point ; statut à clarifier/corriger, pas présenté comme vulnérabilité confirmée.

## Contrôles relus sans défaut identifié dans ce périmètre

- `VerifiedProfile::authenticate` impose la credential présentée au vrai AuditClient puis rôle runner, tenant et namespace exacts avant writer/job/projet/configuration de cette requête (`service/mod.rs:114–169`, `server.rs:506–524`). Aucun fallback vers la clé reconciler ne sert à autoriser une demande utilisateur.
- `supervision_identity` conserve les classes401/403/unavailable, réponse bornée16KiB, parser fermé et validation du protocole ; le client HTTP existant refuse redirects/proxy et HTTPS hors loopback (`runner.rs:159–197`, `:209–241`).
- Paramètres runtime/configuration viennent du profil privé, scope persistant recoupé, journal par profil/tenant et source/export autorisés avec capacités de répertoire. Le registre retenu est revalidé avant accès ; les requêtes n'imposent pas librement origin, image ou credential runtime.
- Submit réserve les bytes exacts avant source/fournisseur et son replay Accepted/Rejected rend le reçu sans résolution Auto/source tournée (`server.rs:633–655`). Les secrets de contrôle/presented sont filtrés après décodage JSON avant réserve ; l'enveloppe credential n'est pas persistée.
- Status/List propagent les erreurs writer au lieu de les convertir en activité absente (`server.rs:538–576`). Les faits affichés restent ceux du journal/control, pas une inspection moteur inventée.
- CLI explicite sans autostart, secrets lus depuis fichier privé et non argv ; les UUID de mutation sont émis avant envoi pour le replay, les erreurs/rejets provoquent un exit nonzero. Les réponses perdues restent un résultat inconnu, pas un refus certain.

## Limites et statut

Défaut connu hors nouvelle découverte : reprise STOP-only dépendant encore du writer complet et des blobs credentials. Root le corrige séparément ; ce rapport ne le revendique pas comme nouveau finding. D5/disarm, provenance de fin normale de l'emitter, gardien TTY, ressources100workers, Docker/Podman et matrice Linux/macOS restent hors qualification de cette lecture.

Root a modifié pendant la revue uniquement les deltas annoncés : getters inemployés retirés, BatchOutcome sans données redondantes et gate admission R2. La revue porte sur leurs chemins relus, sans verdict sur des corrections ultérieures non encore recoupées. Les tests IPC/CLI/writer mentionnés par l'orchestrateur n'ont pas été exécutés par ce reviewer et ne constituent pas une preuve moteur.

Empreintes de clôture de lecture avant les corrections R1/R3 :

| Fichier | SHA256 |
|---|---|
| service/profile.rs | aac35b496451ee4317a3adb1edcbbe1f7f7b8f2585c09d04b82eebbcad54b532 |
| service/protocol.rs | a88c580fbe85d529ed749c35051a088f4da98efe8f63d0ba199dbd754b49a73e |
| service/client.rs | 36572574165cde51379e468391844cd2f8e276110f4e2f8c8335f14e13dafac6 |
| service/mod.rs | 686f96f43fdb691c863e559feb516bee127ce8a26f575dd606e492c876ff6107 |
| service/server.rs | 4925138059f6c63cc75f0a48eb26e2ef2378e5e66ff1460a047366b316168114 |
| service/jobs.rs | 804da43106789a749fe54863a06cdc1ccd065af03fc456cf5692c4cd42e5da10 |
| service/targets.rs | c33b3fecb96d9584c6f7bdc865b6e968c0cca369c1b6a4e26da845f9f9a91869 |
| broker_cli.rs | e42e8edfd1963a11fae0d48d0a01b28e2d3cd6603e9eb9fab0026136fe185e21 |
| main.rs | de0f59953d005ec3a28a8039fedc306bbf22bb602d519ffb942819a54ef06782 |
| runner.rs | ac62f2fcbe278afbfe3ef11b3ade8b78cfe988b9c6181994c1a9cc15674fbf45 |

Ces empreintes décrivent une lecture partielle de sécurité/comportement ; aucune release ni couverture exhaustive n'est déclarée PASS.
