# Arrêt HTTP avec SSE ouvert — 30 octobre 2026

**Correctif accepté** pour gateway/dashboard : drainage coopératif de cinq secondes après SIGINT/SIGTERM, puis retour de main et teardown du runtime. Sans signal, aucun délai de vie du serveur. L'ingress conserve sa borne existante ; aucune action de worker/broker n'est exécutée par ce code HTTP.

| Preuve | Résultat |
|---|---|
| [RED original](logs/red-final.log) | Repos PASS ; gateway et dashboard avec SSE dépassent chacun 8 s après SIGTERM et sont tués/récoltés par leur test propriétaire. |
| [RED renforcé](logs/red-reinforced-final.log) | Même défaut, après maintien des flux plus de 6 s avant signal. Le client SSE n'a pas de timeout total qui pourrait fermer le flux et fabriquer un succès. |
| [GREEN final](logs/green-final.log) | 2 tests PASS : SSE dashboard SIGTERM→exit 5007 ms, gateway 5010 ms ; repos dashboard SIGTERM 1 ms et gateway SIGINT 1 ms. Budget vérifié depuis le signal, y compris après un `wait` prêt ; pas seulement via timeout_at. |
| [Régressions](logs/shutdown-regressions.log) | Ingress CLI 2 PASS et gateway tenants/crash/restart 1 PASS. Ces trois tests ne sont pas réexécutés après les seuls renforcements du harnais. |
| Analyse et revue | Fmt/clippy all-targets puis [clippy ciblé du test final](logs/clippy-http-only.log) PASS ; [revue indépendante](review.md) acceptée, réserve du harnais fermée. |
| [Artefact](artifacts.md) | Fmt final et unique build release PASS, 19,48 s, manifestes Cargo/sources inchangés. Nouveau binaire `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9` ; ancien binaire auth préservé. |

[Design](design.md), [implémentation](implementation.md), [rapport complet](tests.md), [main avant](before-main.rs.txt), [main après](after-main.rs.txt), [test final](after-tests.rs.txt). Les versions RED et GREEN successives restent distinguées ; les résultats répétés ne s'additionnent pas au compteur de tests uniques. Source finale main `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`, test `7d0a817cdfd1b405f0c2704975e05a6e5ca2b2129474504bb099b1e3317cf942`.

Signaux Unix installés de façon fallible avant bind/readiness ; le chemin SIGINT utilise maintenant `SignalKind::interrupt`, pas le fallback non-Unix `ctrl_c`. L'erreur d'installation du handler n'a pas été injectée. Le helper reste privé au binaire : abandonner une future serveur dans une bibliothèque ne suffit pas à garantir la fin de toutes ses tâches réseau.

Les marqueurs étaient préparés avant SIGTERM et lus après : cela ne prouve pas une nouvelle écriture d'audit après signal. La fenêtre de cinq secondes n'est pas une borne temps réel sous suspension OS, I/O/SQLite bloqués ou starvation/teardown. Pas de qualification de tous les clients lents/keep-alive, d'autres plateformes ou du runtime worker après mort du broker. Aucun moteur, modèle, navigateur ou service OS utilisé/installé pour ces tests HTTP.
