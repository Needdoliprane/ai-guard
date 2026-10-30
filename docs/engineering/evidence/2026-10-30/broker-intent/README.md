# Intention immuable — composant logiciel vérifié

30 octobre 2026, worktree `codex/broker-supervision`, base `11eb871db40ca9893a4492547561ec604247257c`. [Rapport d'implémentation au gel](implementation.md) : ses mentions pending sont historiques, avant les résultats finaux ci-dessous et dans le [rapport Luna](tests.md) et la [revue indépendante finale](review.md). Aucun journal, ACK accepté, service ou effet runtime n'est livré par ce composant pur.

## Contrat livré

`broker::intent::ExecutionIntentV1` sépare les champs initiaux du vrai `RunPlan` de ses données `managed` et de son descripteur transitoire de credential. Format1 privé et immuable, distinct des plans1/2/3 : options/demande initiales, identités, input fournisseur et référence de binding. Champs privés, snapshots typés, aucun setter ; sérialisation destinée au futur journal privé. Debug et erreurs sont opaques, y compris la désérialisation directe des snapshots publics.

`from_plan_bytes`, `from_bytes`, `validate`, `canonical_bytes`, `intent_hash` ont des erreurs typées. Plafonds avant parse32Mio pour le plan et256Kio pour l'intention ; plafond canonique256Kio. Nouveau format fermé avant conversion en Value, JSON UTF-8 aux clés récursivement triées, ordre des tableaux et texte conservés. Hash : domaine `ai-guard.execution-intent.v1`, octet NUL, puis JSON canonique. UUID typés sérialisés canoniquement, sans normalisation de chemin/prompt/argv. Golden indépendant `f6f8fa6cc4991a3d03a00655b5d57a5bb186e3e02bb2a1b4ac07adede936c616`.

Versions/UUID non nil, bornes et cohérences initiales vérifiés ; approbation déjà appliquée refusée sans nettoyage silencieux. Choix fournisseur figé et `provider_input` compatibles, prompt exact via le parseur bootstrap existant. Auto/absence restent possibles avec résolution figée ; aucun profil ou credential n'est relu. Les plans1/2 ne peuvent acquérir des sélecteurs gérés. Un ancien plan sans identité initiale suffisante est refusé par cette projection, sans réparation ni modification de sa lecture historique. `sandbox_id` reste une métadonnée indépendante ; ce n'est pas une identité tenant.

La projection ne lit aucun fichier, source de credentials, réseau ou TTY. Une référence de binding ne prouve ni sa présence ni son digest réel. Le hash couvre l'invocation et les références du plan ; le futur journal doit engager séparément Submit exact, périmètre authentifié et bindings durables manuel/fournisseur avant ACK, sans réouvrir les sources après acceptation. Arrêt d'un worker déjà parti indépendant de ses credentials.

## Preuves exécutées

| Contrôle | Résultat |
|---|---|
| RED avant code | E0432 module absent, [log](logs/red-feature-absent.log) ; préparation de test, aucune vulnérabilité runtime déduite. |
| Premier GREEN | Cinq fonctions PASS, [log](logs/green-initial-5.log). |
| Canaris historiques | `provider_authorization`6 + `supervision_contract`5, [log](logs/green-canaries-11.log) ; golden `worker_registry` ciblé1PASS/35filtrés, [log](logs/green-legacy-golden.log). |
| BIR-01 avant correction | Écart P2 de contrat : erreurs serde directes Options clé/enum et Binding clé reprennent le canari dans Display/Debug. [Premier oracle](logs/red-opacity-first.log), [oracle Options](logs/red-opacity-options.log), [oracle final](logs/red-opacity-final.log). UUID Binding, Request et Top étaient opaques. Aucun appel métier ni fuite réelle de credential établi. |
| BIR-01 après correction | Wrappers publics transparents, données strictes privées et erreur fixe. Six fonctions PASS, [log](logs/green-opacity-6.log), golden inchangé. |
| Dernier gel de test | Six fonctions PASS/0FAIL/0ignoré, [log](logs/green-final-6.log), [avant](test-manifest-before.txt)/[après](test-manifest-after.txt) identiques. Plans valides paddés et erreurs typées distinguent les deux plafonds d'entrée ; prompt non-NUL valide et argv borné distinguent le plafond canonique. |
| Formatage/analyse | `cargo fmt --all -- --check` et `cargo clippy --locked --all-targets -- -D warnings` PASS, [fmt](logs/fmt.log)/[Clippy](logs/clippy.log), [avant](static-manifest-before.txt)/[après](static-manifest-after.txt) explicites et identiques. |
| Build unique final | `cargo build --release --locked` PASS34,83s, [log](logs/release.log), cent empreintes Rust/Cargo/toolchain/trois fixtures identiques [avant](release-manifest-before.txt)/[après](release-manifest-after.txt). |
| Revue | BIR-01 CLOSED, aucun constat ouvert au périmètre logiciel pur ; sources/logs/limites recoupés, aucun test exécuté par le reviewer. |

Les six fonctions finales couvrent projection stable malgré mutations de checkpoint typées, changements stables/binding, roundtrip/golden, sélections/cohérences, bornes et diagnostics. Doublon testé au niveau supérieur, champs inconnus aux niveaux options/request/résolution : ne pas attribuer un test de doublons imbriqués à ces assertions. Dix-huit fonctions uniques dans les cibles sélectionnées ; les douze canaris ne sont pas rejoués après le seul correctif d'opacité. Les exécutions successives ne s'additionnent pas. L'horloge Linux a ses [preuves séparées sur le gel précédent](../broker-clock-linux/README.md).

## Artefacts et limites

Binaire `831a3e19854e29f35dade316f71cb463c43faacf49f5f96204277b5ac85b8e1a`, bibliothèque `7edfc144d56f656bcc9980ec68dd83a42e2554b4a4b2bdce210d2fc8c1b6c280`, [empreintes](release-artifact-hashes.txt). Copies privées mode600 dans `/tmp/ai-guard-intent-qualified.egJ3MF` ; archive Cargo/toolchain/src/tests `c1d799704617736900887915facc52da1657c1eb050a62220e838c9436783c0a`. Sources du manifeste revérifiées après build ; artefacts antérieurs préservés. Rapports/logs copiés sans modification d'octets, retours Markdown et lignes vides Cargo conservés hors contrôle d'espaces Git.

**NOT_RUN pour l'intention :** moteurs, modèles/fournisseurs, E2E, navigateur, images/déploiement QA. Les références de source inexistantes du test établissent l'absence d'accès FS, pas un credential valide. **Toujours ouverts :** journal durable/ACK, IPC et identité fraîche, broker permanent, ordonnanceur, gardien, reprise/réactivation, fencing D5 et recette A1/A2/B1. Pas de migration automatique, activation du schéma3, start ou facturation. Le FAIL réel du crash CLI reste ouvert ; aucun gate production fermé.
