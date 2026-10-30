# Arrêt HTTP borné — revue indépendante ciblée

30 octobre 2026. Lecture du delta, des rapports design/implémentation/tests et des preuves locales. Aucun code du dépôt édité ; aucun test, serveur, signal, moteur ou modèle exécuté par le relecteur. Aucun secret réel consulté. Nouvelle gouvernance prise en compte : direction broker/gardien approuvée, sans installation implicite ; ce lot HTTP ne livre pas cette supervision.

## Périmètre et identité

- Baseline `/tmp/ai-guard-http-shutdown-baseline/main.rs` : `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`.
- Candidat `src/main.rs` : `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`.
- API/auth inchangées : `0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8` et `f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee`. La revue API/auth précédente reste acceptée.
- Harnais GREEN avant formatage : `78db32d835644e3f5b39e40e6267317a86533dea336776d79c6d4342cb1130f6`, manifest avant/après identique. Harnais final formaté `tests/http_shutdown.rs` : `e874ca8030caae11629a85983f0f6a47b6d367bb63b6ec68d51768f9e28521ad`.

Hashes source et preuves vérifiés directement. Dépendances pertinentes verrouillées : Axum 0.8.9 et Tokio 1.54.1 ; leurs sources locales ont été lues pour recouper les propriétés d'arrêt, sans build ni consultation externe.

## Verdict

**Correction acceptée pour l'attente SSE indéfinie après SIGTERM dans les deux services, sur le périmètre mesuré. Aucun nouveau P0/P1/P2 de production démontré dans ce delta.** Le comportement RED est reproduit dans les logs conservés puis fermé par une sortie réelle des deux processus vers cinq secondes. Une borne absolue d'arrêt sous blocage OS/IO n'est pas prouvée et n'est pas le contrat annoncé.

## Raisonnement et frontières conservées

- `main.rs:163–180` : enregistrement Unix SIGINT et SIGTERM fallible, effectué avant bind/readiness dans gateway/dashboard/ingress. L'ancien `expect` est supprimé ; contexte fixe et propagation de l'erreur. Le signal reçu est partagé entre serveur et superviseur de drainage, sans timer démarrant à l'initialisation. Le fallback non-Unix reste inchangé, sans qualification Windows implicite.
- `main.rs:183–209`, appels gateway/dashboard lignes 312 et 357 : arrêt gracieux d'abord ; cinq secondes de grâce seulement après résolution du signal. Les résultats serveur restent propagés dans les deux branches. Au dépassement, diagnostic fixe puis retour de main. Ce choix arrête le service avec succès sans prétendre que tous ses clients ont terminé normalement ; les flux peuvent être interrompus.
- Le commentaire sur le runtime est pertinent : Axum crée des tâches de connexion séparées (`serve/mod.rs`, lignes 275–299 et 389–414). Abandonner uniquement la future de serveur ne les annulerait pas toutes. Ici le helper est privé au binaire et main termine immédiatement la branche ; la destruction du runtime élimine les tâches asynchrones quand elles rendent la main. Aucun daemon détaché, nouveau retry, worker stop ou nouveau droit d'exécution n'est ajouté.
- Ingress conserve son ordonnanceur double listener et son délai antérieurs ; seul le constructeur de signal et le commentaire changent. Authentification, rôles, tenant, audit, route et corps HTTP ne changent pas dans ce patch.
- Une décision ou écriture en cours peut perdre sa réponse lors de l'arrêt, comme une connexion interrompue ; le patch n'invente pas d'ACK ni de commit. Les transactions et opérations idempotentes du contrôle restent responsables de la reprise. Aucun résultat de ce test ne requalifie leur résistance à un arrêt à chaque fenêtre transactionnelle.

## Preuves recoupées

Rapport `/tmp/ai-guard-http-shutdown-tests.md`, preuves `/tmp/ai-guard-http-shutdown.rR1hlb/` :

- RED initial `red-final.log`, SHA `0cfc2990590d2fd602b47bc39a5fc961d86240354b0b157ef8fac6e085dd3b97` : idle PASS ; les deux services avec SSE ne sortent pas sous huit secondes et sont nettoyés individuellement.
- RED renforcé `red-reinforced-final.log`, SHA `13bf550fe193ef6dc6334bfebb1a814bacdbc34de43d4c1225a78f84bd0770f1` : même FAIL après retrait du timeout total client et maintien des deux services vivants au-delà de six secondes avant signal. Les anciens échecs de compilation/PATH ne sont pas des RED comportementaux.
- GREEN `green-http.log`, SHA `d591fbe64e9832aa0d2e6df09604b8e0e6dbfe49b5f9ee6b7f349a24b8b97d06` : **2 PASS / 0 ignored**, sortie SSE dashboard **5006 ms**, gateway **5008 ms** ; idle **0 ms / 2 ms**. Mesure depuis l'instant précédant SIGTERM jusqu'au retour de wait : elle inclut l'observation/reap, pas seulement la demande d'arrêt. Les réponses SSE restent détenues pendant wait, sans expiration totale du client ; health et vie des processus sont recontrôlées après six secondes avant signal.
- `shutdown-regressions.log`, SHA `97134c194aff405268fc9750fe82bdd2267c70823eb2dd9dd96db1543e7b2592` : ingress_cli **2 PASS**, gateway_tenancy **1 PASS**, aucun ignoré. Régression ingress avec vrai enfant/flux actif incluse.
- fmt et Clippy all-targets **PASS** (exit0 ; Clippy SHA `cc2a5d64761280e91133e2b47242f8fef6cd2df1f64e8cfc3fc6709ebc9dbd29`). Aucun build release attesté à ce gel de revue.

## Remarque de harnais et limites

**Amélioration d'assertion conseillée, non réfutation du GREEN mesuré :** `await_bounded` utilise une échéance absolue `signal_at + max`, ce qui corrige le déplacement du budget après lecture des marqueurs. Toutefois Tokio `Timeout::poll` essaie d'abord la future interne : si le harnais est réveillé tard et le child déjà fini, un résultat Ready peut gagner malgré l'échéance dépassée. La branche succès imprime la durée mais ne compare pas explicitement cette durée à `max`. Ajouter cette comparaison rendrait l'assertion robuste à ce scénario. Les quatre durées du log présent sont effectivement sous leurs limites : aucune fausse mesure verte établie ici.

Les marqueurs ont été préparés avant SIGTERM et peuvent être déjà tamponnés : leur réception prouve une livraison disponible pendant le test, pas une nouvelle transaction traitée après le signal. Le code gracieux est recoupé séparément ; le test n'affirme pas une admission post-signal.

La documentation Tokio `runtime/runtime.rs:35–47` précise que la destruction attend les travaux bloquants et les tâches qui n'ont pas rendu la main ; elle peut attendre indéfiniment. SQLite/IO synchrone, stdout/stderr bloqué, starvation ou suspension OS restent donc hors de la borne mesurée. Ne pas transformer le retour du helper au bout de cinq secondes en promesse générale de mort du processus. Cette limite est déjà correctement annoncée par les rapports design/implémentation.

SIGINT et erreur d'enregistrement des handlers sont vérifiés par lecture, **pas par faute/test réel dans cette campagne SIGTERM**. Pas de qualification nouvelle des headers, keep-alive, HTTP lent/DoS, charge100, Linux/Podman, supervision après mort du broker ou modèles fournisseurs. Aucun nouvel écart n'est imputé à ces domaines non modifiés.

Statuts : revue/correction ciblée **ACCEPTÉES**, preuves SSE et régressions **PASS** relues ; exécution par le relecteur **NOT_RUN** ; arrêt temps réel général et supervision broker **NON QUALIFIÉS**.

## Addendum final — réserve du harnais fermée

Gel de test `7d0a817cdfd1b405f0c2704975e05a6e5ca2b2129474504bb099b1e3317cf942`, production toujours `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`, rapport de tests `2672e0ecd8f66aad81d2df68bf1a6f28f243256460d0cf3411d423c451ff2676` vérifiés. La branche de succès `await_bounded` compare maintenant explicitement la durée écoulée à `max` et retourne une erreur si elle le dépasse, même si `child.wait` était Ready : **remarque de harnais fermée**.

Le cas au repos envoie réellement SIGINT au gateway et SIGTERM au dashboard ; les deux cas SSE conservent SIGTERM. Cela couvre la branche Unix `SignalKind::interrupt` ; le terme « ctrl_c path » du rapport de tests désigne le signal utilisateur, pas un appel à la fonction `tokio::signal::ctrl_c` sur Unix (celle-ci est désormais réservée au fallback non-Unix). Le nom historique du test au repos mentionne encore SIGTERM, sans modifier les signaux attestés par son code.

Log final `green-final.log`, SHA `3158049e16e0e42ce30f615433f8bab94af59762273b2cc96211e747e88fc92d` : **2 PASS / 0 ignored**, idle dashboard SIGTERM **1 ms**, gateway SIGINT **1 ms**, SSE dashboard SIGTERM **5007 ms**, gateway SIGTERM **5010 ms**. Log `clippy-http-only.log`, SHA `2be1ab3caacc71defd280452a2fc73eb174c26d22c5462d7581f10ab0571d9b8` : Clippy ciblé termine sans warning. Aucun réexamen/rerun des trois régressions ou des 51 tests API n'est imputé à ce second passage.

**Clôture de la revue ciblée confirmée.** La faute d'installation des handlers reste non injectée ; les limites runtime/IO/plateforme et marqueurs préparés avant signal restent celles ci-dessus. Aucune nouvelle preuve release ou supervision broker n'est revendiquée. Les RED et le premier GREEN restent conservés.
