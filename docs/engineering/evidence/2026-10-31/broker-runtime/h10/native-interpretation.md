# H10 — interprétation native avant lecture de QA

Gel source `6c7b63ebcef4a92cadfc2ec95626033f48011576`. Lecture seule ; aucun Cargo, moteur, service, probe, QA, Store, credential ou fournisseur ouvert. Aucun résultat H10 ni cause H9 présumé. Contrats recoupés : `BROKER_SUPERVISION.md:53–69` (deadline native/UTC et observations distinctes) et `:149–155` (permission fraîche, arrêt ciblé, pas de désarmement/terminal inféré). Second regard indépendant sur les clocks et diagnostics.

## Où le slot quitte Ready

`SlotPhaseV1` contient seulement Initializing, Ready, Stopping et Quarantined (`enforcer.rs:348`).

| Source | Condition et transition | Ce que cela établit |
|---|---|---|
| `server.rs:324–344` | Nouveau Register : Initializing, clock=None, usable=false, prepared_once=observe_only. Replay exact conserve l'acteur et sa deadline. | Capacité retenue ; aucune préparation utilisable ou exécution prouvée. |
| `server.rs:346–351`, `:380–411` | Échec création thread/runtime ou réouverture ManagedRuntime : `actor_failed` → Quarantined, stop_failures incrémenté. Réouverture retentée chaque seconde ; runtime/thread non créé peut rester inutilisable. | Défaut de préparation de l'acteur, sans clock ni terminal nécessaires. Une réouverture ultérieure réussie peut conduire à Ready si l'Arm est encore live. |
| `server.rs:421–448` | Tick : sample natif, compteur non régressé par rapport au tick précédent, puis `Arm.validate_clock_check`. `stopping |= !live || actor.stop`. Sous mutex : clock mise à jour si sample Ok, phase Stopping si latch=true, sinon Ready ; usable=true. | Stopping peut venir de deadline, boot/méthode refusé, compteur régressé, lecture clock impossible ou demande Cancel. La phase seule ne distingue aucun de ces motifs. |
| `server.rs:450–463` | Si stopping : appel ciblé `stop_snapshot_owned`, puis Quarantined sur **Ok ou Err** ; observation remplacée sur Ok, compteur d'échecs augmenté sur Err. Attente250ms puis nouveau tour. | Tentative d'arrêt terminée, pas nécessairement réussie ni terminale. Pas de D5 ; l'acteur reste ciblé contre un ancien start tardif. |
| `server.rs:250–262` | Cancel authentifié avec Arm/hash exacts et génération non décroissante : génération remplacée, actor.stop=true, ACK. La phase n'est pas modifiée dans ce handler. | L'ACK exact prouverait la demande anticipée reçue par l'acteur, pas un stop moteur ni un terminal. |

Le latch local `stopping` de `run_actor` ne revient jamais à false : après son activation, les phases alternent Stopping pendant l'appel et Quarantined après son retour. Les clocks continuent à évoluer. À l'inverse, Quarantined issu d'`actor_failed` avant `run_actor` peut devenir Ready après récupération ; le compteur seul n'indique pas le dernier motif. Un mutex empoisonné peut empêcher de publier le nouveau snapshot tout en imposant stopping.

Prepare ne change pas la phase : après validations, il consomme prepared_once et écrit sa clock avant l'ACK (`server.rs:225–245`). Register observe_only peut aussi poser prepared_once sur un acteur existant (`:313–319`) sans prouver un Prepare/start. Côté client, l'état retenu démarre Initializing (`client.rs:258–269`), puis des réponses vérifiées par Arm/hash le remplacent (`:647–674`, `:1132–1150`). `prepared_once` peut aussi être surchargé conservativement en observe-only/retained-cancel (`:290–299`, `:493–503`) : ne pas le présenter comme un fait d'émission moteur.

## Horloges et portée des champs

- **`slot_snapshot.clock`** : clock du status retenu côté client, issue du helper lorsqu'elle existe ; elle peut provenir d'un tick ou d'un Prepare du serveur. La copie sur erreur est sous le mutex du status et après validation structurelle (`diagnostics.rs:119–135`). `clock=None, clock_rejected=false` signifie absent/inconnu ; `None,true` signifie présent mais rejeté par cette validation, pas une panne OS démontrée. `source=helper_slot` n'en fait ni une clock locale fraîche ni l'instant de l'erreur ; les champs prepared_once peuvent inclure les surcharges locales ci-dessus. Âge de réception, epoch et premier motif de transition ne sont pas dans ce snapshot.
- **`native_sample`** : sample local déjà obtenu par le contrôle qui échoue. BeforeClaim et BeforeSpawn renvoient les refus de prédicats, notamment SlotPhaseMismatch, **avant** de lire cette clock (`client.rs:749–768`, `:958–997`) : None est alors attendu. Les samples Prepare sont lus avant le RPC (`:827–856`), donc ne datent pas une erreur transport/refus distant ultérieure.
- **Deadline** : `Arm.validate_clock_check` exige Arm valide, même boot/méthode, compteur >= Arm.clock et strictement < deadline (`supervision.rs:538–546`). UTC ne décide pas l'expiration. `reason=deadline_expired` avec son native_sample vient d'un rejet de ce contrôle local, classifié après identité et régression (`diagnostics.rs:191–211`) : échéance atteinte à ce sample, pas délai physique de stop qualifié.
- Un `slot_snapshot.clock >= deadline`, validé et recoupé avec le **boot/méthode de l'Arm exacte**, établit que cette lecture helper retenue était au-delà de l'échéance. Il ne prouve pas le premier déclencheur : le latch peut venir d'un stop/échec clock antérieur et la clock continuer ensuite. Un sample retenu < deadline ne prouve pas que la clock était encore live au moment de l'erreur. Ne jamais comparer les compteurs d'epochs/méthodes différents.

La priorité des checks compte. BeforeClaim teste phase avant génération, freshness et stop ; SlotPhaseMismatch ne prouve donc pas ces contrôles suivants satisfaits. BeforeSpawn teste les générations avant la phase, mais pas prepared/freshness/stop/clock après celle-ci (`client.rs:999–1044`). `StopSignalled` désigne le watch **local**, également activé sur panne client ou phase nonReady (`:704–709`, `:1145–1148`), pas nécessairement un Cancel durable. Génération ou prepared_once seuls ne prouvent ni Cancel ni start.

## Observation minimale après signal terminal Root

Sans nouveau moteur/probe : projeter seulement le **premier événement d'erreur exact** déjà conservé, avec UUID/seq, phase, Arm ID, stage/reason fermés, deadline, native_sample et slot_snapshot validés ; recouper l'Arm originale (clock, timeout, deadline, scope). Le marqueur HTTP à huit champs omet volontairement reason et clocks : il ne suffit pas pour conclure à l'expiration. Une clock/helper/status interrogé ultérieurement ne reconstitue pas le premier déclencheur et peut refléter récupération ou nouvelle lecture.

Pour affirmer une demande anticipée explicite, recouper la commande Cancel durable et sa livraison ACK exacte au même Arm/hash/génération ; le snapshot actuel n'expose pas actor.stop ni l'identité du Cancel. Pour affirmer un terminal, il faut l'observation du même CID et son traitement durable ; Quarantined, stop_failures ou prepared_once n'y suffisent pas.

Si l'événement enrichi ne distingue toujours pas le déclencheur initial, la donnée manquante est un **motif fermé de première transition** retenu côté acteur (deadline, Cancel, clock refusée/régressée/inaccessible, état indisponible), avec le sample existant applicable et sa provenance. Ce serait un lot diagnostic séparé à proposer, sans lecture d'horloge supplémentaire ni assouplissement de garde/stop ; aucune modification ni nouvelle exécution n'est autorisée par cette note. Une absence de donnée reste inconnue. G09/D5 ne sont pas clôturés par une interprétation de snapshot.

## SHA source

```text
e141119af2c9e91c5023e6df57987629192a0bbd9e657cca1f845afd03e7fa9d src/broker/enforcer/server.rs
92980c856f9cd111105c19abb26291e0923f0284958e9e2f7f066f84cc03e7e5 src/broker/enforcer/client.rs
01bba0106837717065395a0a2221c8bc5523b02ab8615b45c4fb98488aedafec src/broker/enforcer/diagnostics.rs
f48e76fe93756961aab4965a959309158f319d7735ddd25d7e515aa758892ba8 src/broker/enforcer.rs
1a65c3cd5e9bc10cc8ec98df832a073a0fed1f24ae9fdff45a3631aec2f2484c src/control/supervision.rs
```
