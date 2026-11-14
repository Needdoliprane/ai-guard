# Inspection du registre — preuve composée du 14 novembre 2026

[English](README.md) · [47 sélections, argv et empreintes exactes](SELECTIONS.json)

**Inspection, préparation CAS historique et vrai CLI Rust interne passent sur
la source complète `45179db`.** L'opérateur peut conserver explicitement les
métadonnées réservées, examiner le rapport privé historique puis dériver une
opération locale replace/revoke. La transaction A1 finale reste l'autorité pour
identité, scope/CAS et plafond d'un remplacement Session. Elle ne répare pas un
ancien rapport ou une opération ambiguë. [Guide opérateur](../../../../setup/identity/OWNED_SOURCES.fr.md#owned-policy-a1-operator)
· [Contrat API](../../../../api.md#owned-policy-inspection).

## Source et exécution réelle

Commit `45179db23205ff2a534bcadfafd694830f2a62ce`, arbre
`810080f9933ac0f0ef630f3c1b99c600e9f804a5`, archive d'entrée SHA-256
`5379954066640a38ec91753d7adabd65577748145ba087a5347452e82423c1e5`.
La QA admise exécute une seule fois :

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --bin agentsandbox --test kubernetes_waiting_cli --no-run --message-format=json -j1
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets -j1
```

Les deux commandes retournent 0. Le même graphe Cargo frais lie cinq exécutables
distincts : libtest bibliothèque, bin-test agentsandbox, intégration waiting,
agentsandbox normal et ai-guard-provider normal. Les SHA/tailles/tuples sont dans
le relevé typé ; aucun ancien binaire n'est importé ou exécuté sur l'hôte. Deux
textes CLI ont des checks stdin non mutateurs avant Cargo ; le test JSON corrigé
a son reçu formatter distinct lié aux octets exacts. Le formatage n'est pas un
test de comportement.

24 listes réelles vérifient noms et classes ignored avant exécution. Le default
--list inclut des ignorés ; il est corrélé à --list --ignored. Chaque groupe
emploie --exact et tous ses noms, puis --test-threads=1 --nocapture. Seul
cli-ignored ajoute --ignored et reçoit chemin/SHA du CLI normal fraîchement lié.
Le relevé typé donne chaque vecteur d'arguments réel. Les trois CLI ignorés par
défaut ont été exécutés ; aucun sélectionné n'est laissé ignoré et les helpers
hors sélection ne sont pas comptés en plus.

| Groupe | PASS réels | Portée |
|---|---:|---|
| baseline-api | 3 | Vrais heads/scope, refus précoces et observations indisponibles/périmées |
| cli-ignored | 3 | Vrai CLI interne inspect/prepare/apply, conflit retenu et reprise après reçu perdu |
| core | 9 | DTO/SDK de métadonnées fermés, places API réservées, redaction et lecture sans écriture |
| cli | 13 | Parsing/rapport/draft, CAS/plafond/revoke et erreurs de publication |
| e1compat | 2 | Compatibilité ressource/plafond Session et transition terminale |
| a1auth | 3 | Frontières existantes rôle/corps/méthode |
| fs | 3 | Publication protégée, ascendance physique retenue et invalidation persistante |
| kube-contracts | 4 | Un contrat HTTPS/TLS loopback et trois duplex mémoire |
| stdio | 7 | Vrais subprocess provider Rust normal : pipes/arguments/EOF |

Total : **47 PASS uniques, 0 FAIL, 0 sélectionné NOT_RUN**,27 libtest / 13 bin-test /
7 intégrations ;44 ordinaires et 3 default-ignored explicitement exécutés. Les tests
utilisent vrai gateway/SDK/SQLite et opérations FS dans des fixtures possédées.
Les CLI sont ceux d'ai-guard en Rust, pas des sessions Codex ou Claude. Le
dispatch d'attente du provider est un vrai binaire Rust, sans client ou modèle IA.

## Préservation et limites précises

Les 2836 fichiers source gardent hashes, tailles et modes. Sept collecteurs, OOM 0,
fermeture/absence du conteneur exact et flush FD/fichiers/dossiers séparé sont
vérifiés. Aucun fichier régulier de fixture n'est retenu ; l'inventaire primaire
observe neuf répertoires/nœuds de groupe vides. Le tar régulier est vide. Le champ
fixtures.nodes=0 du résumé n'est pas l'inventaire complet ; les deux records
originaux restent inchangés et le SHA de l'inventaire est dans SELECTIONS.
Les fichiers par phase et un extrait terminal borné sont conservés ; la
transcription brute complète n'a pas été persistée ni reconstruite.

L'oracle émet réellement **NOT_APPLICABLE_CASE_ALIAS** sur le filesystem Linux
sensible à la casse. La branche alias physique report/REPORT n'est pas exercée ;
aucune qualification macOS/case-insensitive. Les deux régressions d’ascendance
physique sont distinctes de cette branche manquante.

Les quatre contrats Kube sont un vrai upgrade HTTPS vers un peer TLS loopback
synthétique possédé et trois flux duplex mémoire. Ils ne qualifient ni serveur
API, kubelet, Pod ou cluster. Les sept stdio ne testent ni image worker ni
transport attach. La [preuve Kube antérieure](../kubernetes-attach-composition/README.fr.md)
garde sa source propre de 11 parents. Ce run répète ces 11 parmi 47, sans rejouer les
121 parents be6 séparés ni additionner les comptes historiques.

La QA outillage utilise Linux ARM64 dans Docker, image épinglée, UID 1000,
réseau none, rootRO, caps supprimées/NNP,4 CPU / 9 GiB sans swap et128 PIDs. Le pic
9209622528 octets est la mémoire build/test, pas un usage worker ni une mesure de
capacité 100. Aucun moteur/worker réel, filtrage réseau produit, modèle,
Codex/Claude, cluster, Clippy strict, suite complète ou panne physique/reprise
qualifié. E2/flow16 et fédération Claude restent des branches statiques distinctes,
non intégrées ni qualifiées par ce gel/run. **0/12 gates production fermés.**

## Reçus et historique préservé

| Record primaire | SHA-256 |
|---|---|
| Résultat composé réel | `8da8e0a8a345a7993374f4170234d774771aed97434db75a13dee7639203204a` |
| Flush FD/fichiers/dossiers | `7ea97381c88406a8cb28b1bc1833268f6efabe8eccd4b79ae2172b46d2f1c51f` |
| Formatter du texte JSON exact | `68df78f022cda7c1fa7a963af417ff0748e6442e66984b382eb78acf150b80a2` |
| Flush formatter | `f0aa6dc1fbb32bab276be9026f0cc893b5c96fded8913dde293ce3abe4e5496d` |

R1/e54 échoue en compilation avec trois E0308 avant tests. R2/944 compile/check,
puis 15 parents donnent 14 PASS / 1 FAIL d’oracle et 32 NOT_RUN. Cet oracle trouvait la
sous-chaîne name dans la clé obligatoire control_namespace_uuid ; R3 corrige
seulement le test pour lire les clés JSON exactes. Ce FAIL ne démontre pas une
fuite wire produit. Le compteur FINAL 6 de R2 additionnait seulement les tests PASS des deux
groupes entièrement réussis ; les huit succès individuels du groupe Core en échec gardent leur sortie
primaire. Les anciens records ne sont ni réécrits ni reclassés en green.

La [preuve A1 de 74 parents](../owned-egress-policy-api-cli/README.fr.md), publiée
via `1a9`, conserve sa source testée `db2678d`. Traces brutes, contenu des fixtures,
credentials, chemins privés personnels et binaires opaques ne sont pas publiés
avec ces empreintes.
