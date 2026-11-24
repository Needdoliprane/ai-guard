# Checklist de qualification et de publication

Cette checklist prépare une décision sur une candidate identifiée ; elle ne déclare aucun gate réussi et n'autorise pas une publication. Le périmètre global reste le produit complet local-first/cloud. Une preuve locale restreinte n'autorise pas à annoncer cloud, Kubernetes ou capacité 100 qualifiés. Autorités : [AGENTS.md](../../AGENTS.md), [gates G01–G12](PRODUCTION_READINESS.md), [stratégie de tests](TEST_STRATEGY.md), [état courant](CURRENT_STATE.md) et [preuves courantes](VALIDATION.md).

## Dossier de candidate

Le [programme V1](V1_DELIVERY.md) regroupe les travaux, les matrices et les vérifications d’audit qui alimentent cette checklist.

- [ ] Identifier révision/empreinte, date UTC, responsables réels, artefacts distribués, checksums, versions/digests et entrées de build.
- [ ] Vérifier Dockerfiles et instructions de construction/distribution par le client, y compris préparation/import hors Internet selon le profil annoncé. Aucune image publique préconstruite du projet n'est obligatoire ; traçabilité, versions et preuves concernent les images réellement construites et testées.
- [ ] Fixer les profils annoncés et la matrice Docker/Podman × Linux x86_64/macOS ARM64 ; consigner la recette moteur A1/A2/B1 et les 100 réservations durables de G10. Suivre Kubernetes/cloud et le dimensionnement à 100 workers physiques dans leurs périmètres respectifs.
- [ ] Lier chaque exigence à sa procédure, son résultat attendu/constaté, ses artefacts masqués, sa revue et ses limites. Les comptes/accès externes sont autorisés, jamais inventés.
- [ ] Vérifier les parcours d'installation, diagnostic, identité, lancement/refus, observation, examen/export, arrêt et reprise ; erreurs, données inconnues/périmées, accessibilité et lisibilité sous charge font partie de l'acceptation.

## Revue des gates

| Gate | Preuve à examiner avant décision |
|---|---|
| G01 | Contrats communs, versions supportées et identité des artefacts effectivement testés. |
| G02 | Confinement réel, restrictions effectives et isolation des clients/volumes/autorités. |
| G03 | Original inchangé, copie/export sûrs, destination neuve, concurrence et erreurs de filesystem. |
| G04 | Identité authentifiée, rôles, quotas, approbation unique, refus et panne d'audit sans effet. |
| G05 | Réseau et secrets réels, grants dirigés/révocables, refus inter-client et contournements. |
| G06 | Audit/usage durables, idempotence/conflits, saturation, schéma/migrations, sauvegarde/restauration et rétention. Preuves applicatives sur les [trois profils SQL](TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier). Allocation `billable=false` tant que la facturation n'est pas qualifiée. |
| G07 | Fournisseurs versionnés et sessions réelles, sous-agents réellement exposés, provenance et limites de médiation visibles. |
| G08 | API et dashboard réels, permissions/clients, sécurité HTTP/SSE, export, états d'erreur et accessibilité. |
| G09 | Pannes/reprises gateway, dashboard, broker et moteur ; réconciliation sans perte silencieuse ni suppression de preuves. |
| G10 | 100 réservations durables chez A, refus de la 101e, persistance après redémarrage et quota indépendant chez B. Recette sur moteur avec A1/A2 chez A et B1 chez B. |
| G11 | Infrastructure et transports applicables, identités de workload, TLS/mTLS et réseau cloud effectivement appliqués ; documentation conforme au [guide PKI](INFRASTRUCTURE_PKI.md) incluant procédure d'intervention et rollback. |
| G12 | Licence, notices, redistribution, maintenance, contribution/support, canal privé de signalement des vulnérabilités, dépendances/SBOM/provenance et procédures install/upgrade/rollback. |

Le [contrat canonique d'identité et de configuration](../sso.md) et les [vues par permissions](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) font partie de cette revue : identité locale nominative hors ligne, support OIDC public PKCE S256 qualifié même si désactivé à l'installation, mapping et périmètres côté Rust, délégations/audit vérifiable, administration à 360° autorisée et inférence interne sans repli cloud. Examiner leurs [preuves spécifiques](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier), le profil Keycloak opt-in à quatre comptes et les procédures de découverte/raccordement entreprise. Les clés partagées et les fixtures ne satisfont pas ces exigences.

## Conditions de publication

Le dépôt est public pour les validations GitHub Actions. Le mainteneur décide de publier `v0.0.1` sur une candidate identifiée après réussite des parcours exigés et des gates applicables. Un parcours obligatoire en échec, bloqué ou sans preuve suspend la décision de publier.

- [ ] Chaque gate applicable porte `PASS` avec preuve acceptable ; toute non-applicabilité est justifiée pour le profil avant essais. `FAIL`, `BLOCKED`, `NOT_RUN` et test ignoré ne sont pas des succès.
- [ ] Les défauts bloquants des revues indépendantes sont traités et les régressions pertinentes rejouées. Aucun mock permanent, document de fonction future ou total de tests ne remplace une fonction exigée.
- [ ] Les limites et écarts du périmètre publié sont cohérents entre README, contrats, installation et preuves ; aucun ancien découpage MVP/V2 ne retire une obligation.
- [ ] Le texte [Apache-2.0](../../LICENSE), retenu par le mainteneur, les notices et droits de redistribution sont vérifiés sur les artefacts.
- [ ] Le contrôle avant création du tag et la génération des notices natives refusent toute exception `no-text:`. Pour chaque crate sans texte de licence dans le paquet Cargo, vérifier dans les archives la présence du texte amont, du titulaire de copyright et de l'option retenue pour une licence alternative. [Liste des licences tierces](../../.github/third-party-license-expressions.txt)
- [ ] Vérifier le canal GitHub Security Advisories de la réception à la réponse. L’API GitHub a confirmé l’activation du signalement privé le 22 novembre 2026. Avant publication, tester l’ouverture d’un avis de recette en brouillon, l’invitation d’un rapporteur et la réponse avec les comptes autorisés. [SECURITY.md](../../SECURITY.md) · [FR](../../SECURITY.fr.md)
- [ ] La décision finale nomme ses responsables réels, sa candidate et son périmètre, sans mention « production-ready » au-delà des preuves. La publication et les opérations externes disposent du mandat correspondant.

Les rapports historiques restent conservés et identifiés. Le dossier final indique changements, commandes/résultats constatés, critères acceptés, limites et blocages précis, sans joindre de secrets ni de clés privées.

## CI GitHub et brouillon de release

- Les pushes sur `dev` et `main` lancent la vérification du format (`cargo fmt --all -- --check`), les tests unitaires Rust et frontend, les régressions de l’agrégateur fournisseur, l’autotest du conducteur TTY, les tests de prévol `ai-guard-diagnose`, le garde des licences tierces (`examples/third_party_notices.rs`, liste d’expressions relue dans `.github/third-party-license-expressions.txt`) et la compilation du frontend. Clippy n’est pas encore dans la CI : un lot distinct ramène d’abord la base à zéro avertissement. La CI vérifie la version Cargo et la révision déclarées par `agentsandbox version --json` avant qu’un run vert puisse autoriser un tag. [CI](../../.github/workflows/ci.yaml) · [Recettes locales](LOCAL_CLOUD_TESTS.md)
- Depuis **Actions → Build tag and prepare GitHub Release → Run workflow**, choisir `main` et saisir le nouveau tag. La CI du dernier push sur le commit exact doit être terminée avec succès. Le tag doit correspondre aux versions Cargo et frontend du commit. Le workflow crée le tag, puis construit depuis sa référence. Un push Git de tag ne lance aucun build dans ce workflow. [Workflow manuel](../../.github/workflows/release.yaml) · [Déclenchement manuel GitHub](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)
- Le tag annoté lie le nom, le commit et l’identifiant du run. Si l’accusé de création de la référence est perdu, relancer le job du même run. Le retry retrouve l’objet exact et vérifie l’annotation. Le build épingle le SHA de l’objet et celui du commit. Le brouillon contrôle aussi le SHA de l’objet avant sa création et chaque envoi. Une réponse perdue avant la création de la référence peut laisser un objet annoté sans référence. [API des tags GitHub](https://docs.github.com/en/rest/git/tags)
- Pour empêcher aussi la création des tags depuis Git par des comptes disposant de droits d’écriture, configurer une règle de dépôt qui réserve le préfixe de release à l’autorité autorisée. La configuration du workflow borne le chemin de build à son lancement manuel. [Règles de tags GitHub](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets#restrict-creations)
- Avant de créer une archive native, le workflow exécute `agentsandbox version --json` sur le binaire copié. Il vérifie le composant, la version demandée et la révision embarquée. Avant tout appel GitHub, le job de brouillon vérifie localement les sommes SHA-256, `BUILD-INFO` et la concordance des octets de `LICENSE` de chaque archive avec le commit vérifié. Chaque archive native doit contenir une entrée régulière, non vide et exécutable par le propriétaire pour chaque binaire déclaré par Cargo au commit vérifié. Le job en lecture seule `archive_checks` réutilise le CLI construit par `metadata` et exécute `agentsandbox release verify-archive` sur les quatre archives. Il exige un seul répertoire racine et des chemins uniques. Le brouillon compare l’empreinte du fichier de sommes qu’il recalcule à celle du job de vérification avant les appels GitHub. Chaque membre est un fichier ordinaire ou un répertoire. Tout parent est un répertoire présent dans l’archive. L’empaquetage désactive les entrées AppleDouble ajoutées par le tar de macOS (`COPYFILE_DISABLE=1`). L’archive frontend doit contenir `index.html`, au moins un fichier JavaScript sous `assets/` et le fichier `licenses/vite-LICENSE.md` copié depuis le paquet Vite verrouillé. Les archives Linux x86_64, macOS ARM64 et du frontend sont jointes au brouillon avec `SHA256SUMS`. Le brouillon indique le commit et le lien du run CI. La publication demande la revue de la présente checklist.
- **Provenance et inventaire (décision du 21 novembre 2026).** Le job `metadata` s’exécute avant la création du tag : si le manifeste ou le SBOM ne peuvent pas être produits, aucun tag n’existe. Il construit le CLI depuis le commit vérifié, lance `agentsandbox release manifest` puis `release verify --source-only`, et publie `release-manifest.json`, `sbom.cdx.json` et la somme propre à l’outil dans l’archive `…-release-metadata.tar.gz`, avec le SBOM CycloneDX 1.6 isolé. Le `SHA256SUMS` de la release couvre toutes les archives et le SBOM ; les archives sont liées au manifeste par ce seul fichier. Chaque archive porte un `BUILD-INFO` avec tag, commit, run et dépôt (et la cible pour les archives natives) que le brouillon compare octet par octet. Les archives natives contiennent `THIRD-PARTY-NOTICES` (crates de `Cargo.lock`, textes de licence), vérifié non vide. Le générateur d’AC de démonstration (`ai-guard-demo-pki`) est exclu des archives par la liste `EXCLUDED_BINARIES` du job `verify` (il reste compilable localement) ; la liste des binaires livrés est dérivée de Cargo moins cette exclusion, et un test Rust refuse une exclusion périmée. **Aucune signature cryptographique, attestation ou provenance SLSA n’est produite** ; cette limite et la procédure de vérification sont publiées dans le [guide d’installation](../setup/release/install.fr.md) ([EN](../setup/release/install.md)). Le test Rust `tests/release_workflow_guard.rs` vérifie les membres sur de vraies archives de test et le câblage des jobs sans appeler GitHub.
- **Notices tierces.** Au 22 novembre 2026, les quatre paquets Cargo sans texte de licence ont un texte amont enregistré à la révision exacte du paquet. Les notices natives indiquent que la distribution retient l’option MIT pour `r-efi` 5.3.0 et 6.0.0. Leurs fichiers `AUTHORS` contiennent le texte MIT et les titulaires. `rsqlite-vfs` 0.2.0 reprend la licence MIT de la racine du workspace. `keyed_priority_queue` 0.4.2 reprend son fichier `LICENSE.md`. Pour chaque ligne `supplement:`, le garde vérifie l’empreinte SHA-256 et exige un texte UTF-8 non blanc de 256 Kio au plus. Le garde refuse les expressions sans revue et les crates sans expression ni texte. Le job `metadata` et la génération des notices natives passent `--for-release` et refusent toute entrée `no-text:` avant de lire les dépendances. Les commandes de développement sans cette option conservent les exceptions revues. [Liste et provenance des textes](../../.github/third-party-license-expressions.txt)

La recette locale sur les archives CI de `1c9e8e01` vérifie que le
`LICENSE` Linux x86-64 correspond au commit et que les notices Linux
et macOS ARM64 sont identiques. Les quatre textes complémentaires
figurent dans leurs blocs attribués et les deux choix MIT pour
`r-efi` sont explicites. Le reçu de contenu ne couvre pas la revue
des droits de redistribution.


**Dépendances Rust et frontend.** Le garde de licence de `26743398`
accepte les expressions et textes des 337 crates liées et produit
les notices natives. Le manifeste source et son SBOM reflètent
`jsonwebtoken 12.0.0` et `crypto-bigint 0.7.11`. Sur la base RustSec
`1d328b6b`, `RUSTSEC-2023-0071` reste signalé pour `rsa 0.9.10` via
`openidconnect`. La revue du chemin OIDC a identifié la vérification
par clé publique et le client public. L’avertissement de maintenance
sur `chrono` reste ouvert. L’audit npm du lockfile frontend inchangé
ne signale aucun avis au relevé du 24 novembre 2026.
[Source RustSec](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)

**Paquets système de l’image de contrôle.** Le rapprochement du 24
novembre 2026 couvre 65 paquets source Debian Bookworm installés dans
l’image ARM64 de `7e3a6166`. Pour les entrées résolues avec version
corrective, aucune version installée n’est inférieure à cette version
selon `dpkg --compare-versions`. Le relevé contient 126 entrées
ouvertes et une indéterminée à examiner pour les binaires embarqués.
Les bases épinglées et les paquets résolus par APT au build sont des
entrées distinctes. Refaire le relevé sur les images finales par rôle
et architecture pour un gel de release.
[Debian Security Tracker](https://security-tracker.debian.org/tracker/)

- **Recette Linux x86_64 manuelle.** Le workflow [linux-qualification](../../.github/workflows/linux-qualification.yaml) lance, sur des runners GitHub `ubuntu-24.04`, les sélecteurs ignorés de la recette trois workers A1/A2/B1, de l’arrêt explicite, de l’export (Docker), du confinement (Docker) et de la persistance du terminal, avec Docker rootless et Podman rootless séparés. Pour l’AppArmor d’Ubuntu 24.04 il charge d’abord un profil `userns` limité aux binaires du moteur et ne bascule sur le sysctl `kernel.apparmor_restrict_unprivileged_userns=0` qu’en cas d’échec, en consignant avant/après. Il téléverse uniquement une liste blanche de fichiers (inventaire du moteur, reçus JSON nommés, journaux, `status.json` avec `sysctl_fallback_used`), après un balayage qui refuse tout motif de jeton ou de clé ; jamais l’arborescence des fixtures. L’entrée facultative `release_tag` n’accepte qu’une release **publiée** (un brouillon n’est pas visible du jeton en lecture), validée par la même expression que le workflow de release, et refuse de s’exécuter si le tag ne pointe pas sur le commit lancé. Non couvert : confinement Podman (marqueur d’autorité à admettre séparément), fournisseurs, MySQL, Kubernetes, macOS. Aucune exécution n’a eu lieu à la date de cette modification ; un run vert ne vaut preuve que pour sa révision, son runner et ses versions consignées.
- Si le build ou le dépôt d’archives échoue après création du tag, relancer les jobs échoués du même run dans le délai de rétention de sept jours. Un nouveau lancement refuse le tag existant. Pour le brouillon de ce run, la reprise vérifie le tag, le commit et le marqueur du run, compare les fichiers déjà présents aux artefacts vérifiés, puis joint les fichiers absents à l’ID exact du brouillon sur `uploads.github.com`. Publier après la fin du run. Une publication manuelle du même brouillon pendant une requête de dépôt peut la devancer et sera détectée au contrôle suivant. Un brouillon étranger, une release déjà publiée lors du contrôle, un dépôt incomplet portant déjà le nom attendu ou un fichier divergent bloque la reprise. Dans ces cas, conserver le brouillon et les preuves du run pour inspection.
