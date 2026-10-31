# Revue indépendante PR1–PR4 — adaptation lifecycle et diagnostic préclaim

Worktree exclusif `broker-service`, base `99238e3f38ac851a0a98813ec24ef9c5a2cea950`. Lecture seule des neuf fichiers ci-dessous ; aucun Cargo, moteur, fournisseur, Store, QA ou credential ouvert et aucune source modifiée par le relecteur. Contrats/gouvernance conservés. Sous-revue indépendante supplémentaire des deux fichiers natifs, mêmes SHA après rustfmt.

## Conclusion source après correction et gel PR4

**L'écart bloquant de conservation d'une observation partielle a été reproduit RED, corrigé puis vérifié GREEN. Aucun P0/P1 restant identifié dans ce périmètre après correction.** La suppression d'une lecture Full d'adaptation est établie par le delta ; aucune mesure de gain, cause H4/H5/H7, correction d'échéance ou qualification G09 n'en découle. Le point de libellé diagnostic PR1 est traité dans PR2. La revue source est distincte des validations Luna détaillées plus bas.

### Adaptation V3→V4

`src/broker/journal.rs:1018` prend le Mutex Writer, conserve `check_writer`, puis appelle `status_locked`. Sa branche Accepted effectue un seul préload Full strict. Le nouveau port interne adapte à partir de ce résultat vérifié, sous le même verrou jusqu'à la publication et au postload. Il ne reçoit ni job approuvé en cache ni preuve de durabilité de l'appelant. `writer.rs:873` remplace seulement l'ancien load d'adaptation extérieur par ce port ; le backend reste Full, aucun Stops→Full ajouté.

Pour un head V4, `expected` doit toujours être le head courant, avant adaptation, y compris pour des bytes historiquement identiques. Les événements et leurs ACK sont clonés intégralement depuis le V4 chargé ; l'appelant lifecycle ne peut pas les remplacer. `BrokerCheckpointV4::new` conserve validation de forme, limites et collisions commande/event/export ; hors V4, `validate_command_ids(next, &[])` demeure avant validation du hash attendu. UUID nil avant lecture et contrôles V4/IDs avant hash attendu correspondent à l'ancien adaptateur. Les ports Journal publics gardent leur propre ordre de validation et le refus de downgrade V4→V3.

Le corps extrait `advance_loaded_checkpoint` (`journal.rs:1068`) est le corps antérieur après préload : contrôle de contexte, bytes/hash, replay historique et CAS, transitions, usages/caps, publisher physique et floors. Ses deux seuls appelants gardent le même Mutex et lui passent un load du même Journal. Après vraie publication, floor/observed sont enregistrés avant le postload strict (`:1120`) ; un binding devenu indisponible ne produit pas un ACK silencieux. Le publisher, les fsync des fichiers/répertoires/parent, la confirmation Full pré/post, les règles legacy, le budget, la queue et les issues Unknown ne sont pas rendus permissifs par cette extraction.

Tests écrits relus : `tests/broker_outbox.rs:506` ajoute le rejeu historique V3, un ACK d'événement avant update lifecycle, comparaison sérialisée complète des events/ACK, ancien expected V4 refusé sans nouvelle révision ; downgrade et collisions restent vérifiés. Le canari Full postpublication existant passe par le même corps extrait. Cela couvre les garanties pertinentes, sans qualifier une panne électrique ni un gain de temps.

**Constat bloquant traité — conservation des observations.** L'ancien adaptateur faisait `Journal::load`→`status_locked`, qui marque `observed_jobs` pour un job existant Interrupted/ReservedInProcess sans accepted.json (`journal.rs:581–645`). PR1/PR2 appelait directement `load_locked` ; dans ce cas il échouait à la lecture d'accepted.json avant cet enregistrement. Recette transmise puis implémentée dans `journal/tests.rs:210` : réservation durable, drop ticket puis Journal, reopen sans consultation du job ; appel du port lifecycle → Interrupted ; rename exact du seul job fixture hors `jobs`, réservation préservée ; `status` puis `reserve` du même UUID doivent rendre Conflict, sans recréer de job.

Luna a obtenu un **RED réel**, compilation OK, RC101 sur l'assertion `status→Conflict` (`/private/tmp/ai-guard-journal-partial-red.GgRDO1`). Le log n'affiche pas la valeur retournée : aucune valeur None/New n'est présentée comme observation d'exécution. Le startup service par recovery report conservait déjà les observations ; aucun bypass runtime n'a été déduit de ce constat. Le correctif PR3 est exactement le match `status_locked` avec la conversion de `Journal::load` (Accepted→job, absent/rejected→InvalidInput, autres→Interrupted), toujours sous le même Mutex. Diff source contre le snapshot RED relu : ce remplacement seul ; branche Accepted avec un Full load unique, même adaptation/head/IDs/hash/tail/postload. Canari inchangé **GREEN RC0**, puis test V4 rejoué GREEN. PR4 ne formate que ce nouveau test ; delta de whitespace relu, aucun comportement changé.

### Diagnostics fermés avant claim

`enforcer/client.rs:732` partage un corps unique avec le wrapper public `check_ready_for_claim`. L'ordre court-circuit reste health → lock → acknowledged → déjà prepared → phase → générations → fraîcheur de 750 ms → stop → native clock/sample → validation d'Arm. Les codes publics restent Unhealthy ou NotReady selon les mêmes conditions ; GenerationChanged BeforeClaim n'hérite pas du Conflict propre à Prepare. Même unique sample natif, mêmes checks, aucune nouvelle validation/RPC sur succès. `register` et `register_current` sont inchangés.

`enforcer/diagnostics.rs:68` ne valide l'Arm que dans le `map_err` suivant l'échec de Register (`executor/helpers.rs:114`). Arm validée et UUID canonique/non nul avant copie d'identifiant/deadline ; reason fermé contenant l'EnforcerError existant, aucun sample inventé, runtime-name, invocation ou texte d'erreur. En cas d'Arm invalide, aucun diagnostic natif à métadonnées douteuses n'est construit. Types Serialize/Error sans Deserialize, Display/Debug opaques ; pas de nouvelle autorité.

Les contextes Root sont des variants fixes, conservés par downcast sans sérialiser l'erreur arbitraire. `broker_step` est optionnel dans le payload existant v1 ; effet toujours `unknown`. Les checks de génération/Ready restent aussi après écriture pending et immédiatement avant l'appel central (`executor/journal.rs:194–200`). Le chemin D3 existant conserve FailureObserved/Unavailable, masquage avant outbox V4 et ACK exact, sans nouvel état terminal, fencing ou libération quota.

**Point PR1 traité en PR2.** Un second Ready échoué dans `rpc_checked` peut porter un contexte intérieur ReadinessBeforeClaim puis un contexte extérieur du même enum. Le downcast choisit cette frontière extérieure ; l'EnforcerStageFailure intérieur conserve le reason BeforeClaim plus précis. PR2 renomme ArmAcknowledgement/ClaimAcknowledgement en **ArmExchange/ClaimExchange** et documente explicitement que la frontière ne prouve ni envoi ni ACK (`executor/diagnostics.rs:9–21`). Le delta final des deux fichiers Root ne change aucun contrôle. Ce libellé traite le risque de lecture trompeuse ; aucune preuve d'effet ne doit être dérivée des contextes.

## Validation et limites

Revue source et `git diff --check` : **PASS** dans le périmètre. Tests natifs écrits relus : ordre/raisons Ready, codes publics, métadonnées Register invalides/canoniques, absence de texte privé ; test Root de frontière fermée sous contextes privés. Les tests de classification ne prouvent pas une émission réelle du nouvel événement.

Logs Luna lus : PR1 `/private/tmp/ai-guard-pr1-targeted.bHdR8Q`, 9 diagnostics + 2 prédicats natifs + 1 Full postload + 1 V4 = 13 fonctions PASS, RC0. PR2 `/private/tmp/ai-guard-pr2-final.fI5wMF`, 2 fonctions historiques (lifecycle wire et promotion V1→V2) PASS, puis 3 unités Root rejouées, RC0 ; les rejeux ne sont pas additionnés. PR3 `/private/tmp/ai-guard-pr3-final.4G2xpZ`, nouveau canari partiel 1/1 et V4 rejoué 1/1 PASS, RC0. **Total : 16 fonctions uniques.** PR3 fmt-check FAIL seulement sur le format du nouveau test ; PR4 corrige ce format, sans nouveau rejeu. Aucun test exécuté par ce relecteur.

**Annexe finale PR4 :** dans `/private/tmp/ai-guard-pr4-final.cAd8vX`, fmt-check, Clippy strict all-targets et build release **PASS, trois RC0 lus**. Le log release constate 6 min 20 s. Vérifications Luna : 158/158 avant, après et snapshot, aucun non-OK ; relevés avant/après identiques. Les relevés Root indépendants `/private/tmp/ai-guard-pr4-root-verify.0xlg9R/{source-verification,checkout-verification}.txt` confirment chacun 158/158 OK. Les deux binaires privés archivés ont été vérifiés par SHA-256, sans exécution : `agentsandbox` `ec99e7817efc53ccb912d99bd5d5130c24bc89c9e7a59ef5d24cfb782338e9f6`, `ai-guard-provider` `e50f668b1aafd5928df9b0ff364348b18e5514883308db2d2dabc045743046c9`.

Le rapport H7 `/tmp/ai-guard-runtime-h7-core-diagnostic.md` est lu comme preuve sélectionnée : frontière après Arm worker ACK, avant claim, cause exacte non établie. **H6/H7 restent FAIL ; aucun H8 ni autre essai runtime pour ce lot.** Émission réelle de ces nouveaux diagnostics, effet runtime et gain temporel : **NOT_RUN / non mesurés**. La compilation ne qualifie ni démarrage, ni deadline autonome, ni reprise réelle. G09 reste FAIL ; aucun gate fermé.

## Sources finales PR4 relues — SHA-256

```text
bbf829317219756e075cfa18e5d55f88ee54886de5ba99ec82a8cd70613c36ab src/broker/journal.rs
89958ca42855383cdf33fce9a402aef1a5ad06367112deb1994b4238319935af src/broker/journal/tests.rs
47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7 src/broker/writer.rs
538d89036298c15c6d1d22ab01afa80920f293529e1875be2912f4e741f3ec7c tests/broker_outbox.rs
7e236df040453af0ae03dc91f9f7c2072602515ef78522cebd8cb9a590a64d8d src/broker/enforcer/client.rs
dfabc58b46fecd812223b5eb71b40ae8115e0e62116c63f9b6c36dcdf10c13fe src/broker/enforcer/diagnostics.rs
b3a913e355d5a50f1c84f2217be61eb31fc9de754487f6c59811bc2cc501833c src/runner/broker/executor/helpers.rs
780600599b5dc05fe2fb651b8cf339d12f1db81f3130a9927f1572e69dad5f6c src/runner/broker/executor/journal.rs
db6b6eee4951bb3afa5839d2075f689b74244e65e83f4c7cc8fe6b1b922d44d8 src/runner/broker/executor/diagnostics.rs
```
