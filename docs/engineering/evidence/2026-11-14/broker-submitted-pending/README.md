# Broker Submitted/Pending — preuve logicielle du 14 novembre 2026

**Neuf régressions ciblées PASS sur `a4340c4f293eb459a168c98d33386b44d38ad1fa`.**
Contrôle du format des trois sources courantes, compilation des tests et check
toutes cibles RC0. Ce résultat établit le scénario logiciel chaud ci-dessous,
sans clôturer D5, la reprise générale ou un gate de production.

## Besoin et comportement établi

Le broker retenait déjà l'original avant le premier write possible, refusait
une nouvelle tentative et pouvait nettoyer un Final déjà durable. Il manquait
la continuation lorsque le Pending était audit-ACKé, le vrai retrait gardien
Q acquis, mais que le wrapper vérifié était perdu avant Final F. La baseline
reproduit précisément cette perte de wrapper ; ce n'est pas une perte filaire
de l'ACK et ce n'est pas un terminal de worker.

Le raccord conserve le même owner Submitted et le même Job guard. Il revalide
Pending, son ACK, l'identité, les bytes et l'origine physique du Journal ;
consulte l'état réel du registre Final ; reprend l'entrée déjà possédée ou
obtient le Q authentifié lié à l'original. L'entrée réellement non enqueued
reste retenue pour la reprise ; InFlight et les issues inconnues ne deviennent
pas une nouvelle autorisation. Un Final déjà consommé est relu dans le Journal
original, même si le registre ne le conserve plus.

La fin exige encore F durable et Q concordant, puis retrait du slot local
présent et clear du même keeper sans await entre ces deux derniers actes.
RetireUnused n'est pas retransmis. Un snapshot public de mêmes bytes mais
d'un autre core, une absence de slot ou une simple observation Verified ne
reconstruit aucune preuve. Le rejeu conserve Final et son événement central
unique ; génération, intent/operations, admission fermée, worker/quota/usage
restent inchangés dans les scénarios.

Les fixtures utilisent les vrais producteurs Journal/SQLite, writer, gateway,
IPC et Q natif existants. `SyntheticReady` remplace explicitement l'observation
du runtime métier ; aucun worker, Docker/Podman/Kubernetes métier ou client
fournisseur n'est lancé par cette sélection. Une preuve du gardien local ne
vaut pas terminal moteur ni libération de la réservation du worker.

## Gels et comparaison

| Étape | Source et résultat réel |
|---|---|
| Baseline | `0186cceee61bb3a4990d52bf6629b61546baa265` : format ciblé et build RC0, six listes réelles ; un parent exécuté, RED101 après un unique marker de setup confirmé et l'assertion de convergence prévue. Canaris et check toutes cibles non sélectionnés. |
| Premier GREEN tenté | `a48fc58180c86ee25dc6d9be84bdb3c34c91b263` : build RC0, 20 listes/neuf actifs ; p01–p05 PASS, p06 stack overflow/abort134, p07–p09 et check toutes cibles NOT_RUN. Lot global non vert. |
| Sonde diagnostique | `98b6bbfd14ac8dd7b29a9bb504c943618e27f02c` : leaf de layout PASS, canari étranger stack overflow/abort134. Dernier marker `foreign.snapshot.enter`, sans frame précis démontré. |
| Résultat final | `a4340c4f293eb459a168c98d33386b44d38ad1fa` : neuf parents PASS, zéro échec/incomplet/ignoré sélectionné et check toutes cibles RC0. Ordre réel p06,p01,p02,p03,p04,p05,p07,p08,p09. |

Le scénario p01 conserve son helper, son setup authentique et son oracle entre
baseline et final. L'ancien code exigeait déjà F avant settlement ; le nouveau
code traite Pending+Q sans F. La comparaison démontre cette régression
logicielle du même scénario. Les arbres complets `0186`→`a434` diffèrent sur
64 fichiers, notamment d'autres intégrations et Cargo : ce n'est pas une
expérience globale à une seule variable et aucun PASS n'est transféré à un
autre arbre par égalité de nom de test.

Par rapport au public `c6ec590b5cf4308fb35b50afef6ad0c6082b7377`, le candidat
final change exactement les trois sources ci-dessous. R1→R2 change uniquement
le test étranger : le même Journal copié est donné à `WriterClient::start`,
puis le snapshot est lu par sa vraie queue `RecoveryStatus` au lieu de l'appel
synchrone dans le poll du test. Origine étrangère, mêmes bytes et toutes les
assertions Q/keeper/Final/admission sont conservées ; aucune taille de pile
n'est augmentée. Le PASS indique que cette variation rétablit le canari,
sans identifier le frame fautif ni démontrer un correctif de pile produit.

Le FINAL brut conserve `context_variation=true`,
`precise_cause_frame=UNCONFIRMED` et `product_fix_demonstrated=false`.
Ce dernier champ porte sur l'expérience diagnostique de contexte ; il ne
retire pas les neuf résumés libtest PASS vérifiés par Root et n'autorise pas
à présenter la variation comme une correction de pile du produit. La réponse
réussie du writer atteste la fin de sa lecture synchrone ; son Drop ferme la
queue mais n'est pas une preuve de join du thread. Les erreurs/timeouts ne
sont ni annulation prouvée de cette lecture ni preuve de drain.

## Sélections exactes

Préfixe commun de tous les FQN :
`broker::service::server::retirement_settlement_tests::`.
Les 20 listes sont des contrôles d'enregistrement, pas 20 tests exécutés.
Les neuf FQN ci-dessous ont chacun un vrai résumé libtest 1 PASS ; ils ne sont
pas additionnés aux cinq PASS de R1 ou aux résultats du 13 novembre.

| ID | Suffixe exact du FQN | Oracle principal |
|---|---|---|
| p01 | `same_original_submitted_pending_recovers_q_then_final_without_new_retire` | Pending et Q authentiques, wrapper perdu avant F ; convergence exacte, Final/event uniques, retrait local et rejeu stable. |
| p02 | `same_original_submitted_final_settles_after_lost_q_without_new_final` | F déjà durable, wrapper Q perdu ; cleanup sans réadmission Final. |
| p03 | `same_original_not_queued_final_retries_its_retained_input_before_another_q` | Le vrai input non enqueued est conservé puis repris, sans Q de remplacement. |
| p04 | `same_original_inflight_final_keeps_its_owner_then_consumes_completed_without_readmission` | InFlight garde l'owner ; le Completed réel est consommé sans réadmission. |
| p05 | `same_original_stale_pending_reads_consumed_final_with_closed_registry_without_readmission` | Snapshot Pending ancien et registre fermé : F consommé relu sur la vraie source avant cleanup. |
| p06 | `same_public_final_from_foreign_core_cannot_query_or_clear_keeper` | Mêmes bytes publics étrangers ne donnent ni Q original ni clear du keeper. |
| p07 | `absent_local_slot_without_opaque_completion_keeps_original_keeper` | L'absence de slot seule conserve l'inconnu et l'original. |
| p08 | `actual_retained_verified_input_prevents_another_query_wrapper` | Un vrai input déjà retenu interdit le wrapper de remplacement. |
| p09 | `confirmed_cleanup_after_cancel_preserves_generation_and_closed_admission` | Cleanup confirmé conserve génération et admission fermée. |

## Source et reçus vérifiés

| Source testée | SHA-256 du fichier |
|---|---|
| `src/broker/service/preclaim.rs` | `361ddce95454a86a801634b98951176679237bc54ab291ce70332d5c998a117d` |
| `src/broker/service/retirement_settlement.rs` | `a28441a1ed02661542a51f565736be6fcf7d662a53c27febdbe3de5b640b4c51` |
| `src/broker/service/retirement_settlement_tests.rs` | `0bc89a58002102aa4b083e96f1acbf74964f83c004692b31f0504dc7fba16b1d` |

Reçus primaires conservés en privé ; aucune fixture, sortie brute, binaire,
credential, chemin personnel ou donnée utilisateur n'est publié :

| Pièce | SHA-256 |
|---|---|
| Baseline `ROOT_VERIFIED_RESULT.json` | `0dd0719d7558e88c690cd89321316dabf300e4f657235d40e849d5623b2c696c` |
| Baseline `ROOT_HOST_FLUSH_VERIFIED.json` | `83b3c3ddad3db7abe77849ff4924734e1ce5fb0688fabc219f4af691e663b74d` |
| R1 `ROOT_VERIFIED_RESULT.json` | `e3a76e897893b9af16c177e39eaedd9f12bdac460d2a260c6420e171983ef035` |
| R1 `ROOT_HOST_FLUSH_VERIFIED.json` | `0838364b0c1c5c494862ada2dfd15d8a7e2440e231ff38538133c7392d911c75` |
| Sonde `ROOT_VERIFIED_DIAGNOSTIC_RESULT.json` | `7f4af48f0a7813e77bd5bf6bba4482f19f8adecfc90105ff567dee83d3b4c131` |
| Final `ROOT_VERIFIED_RESULT.json` | `bae9286e238acd66522c0b17854803d972ed8f1977140800853f4ac90b557577` |
| Final `ROOT_HOST_FLUSH.json` | `90593c28e88e577debe872e74d557d735d04dd55403af44d9d11c6f3c433c600` |

La source d'entrée a pour archive
`44739a474238e3aab78f355a8f3f5794a6bfa55499536413026edd1311d50e32`.
Les 2842 fichiers après exécution conservent exactement hashes, tailles et
modes ; l'archive de collecte post-source distincte porte
`99187d309f600740b73db413d78d9e02f0275d8d6669356abd0f0137fa8baeb5`.
La vérification du format courant traite les trois fichiers en édition 2024,
sans enfants et sans mutation source ; aucune preuve historique de formatter
n'est transférée. Le check toutes cibles a zéro erreur et 169 avertissements,
sans message de warning dans les trois fichiers modifiés. Clippy strict est
NOT_RUN ; une compilation réussie ne supprime pas les avertissements globaux.

L'inventaire final contient 96 fichiers réguliers et 60 répertoires, soit 156
nœuds, zéro socket exclu et 2117562 octets de payload régulier. Archive privée
de fixture 2201600 octets, empreinte
`3baa2d0f4a7bc8c3a2adff67e8e8749be52b01dfc5070f32ef46e79266f84b28`.
Le pic cgroup est 8251834368 octets ; oom, oom_kill et oom_group_kill valent 0,
et stopped_oomkilled=false. Le scan complet n'observe aucun processus cible,
sans prétendre être atomique. Stop/remove et contrôle d'absence du CID exact
réussissent ; le 137 du holder après stop n'est pas l'exit d'un test ni un OOM.
Le flush Root séparé vérifie 633 fichiers/10 répertoires. Ces observations
qualifient la clôture de cette QA, pas une procédure de panne physique.

## Limites et suite obligatoire

Le résultat est une preuve logicielle chaude dans le même processus, sur
l'original retenu. D5/fencing global, Unsubmitted sans permis distinct,
reprise froide complète, familles namespace/restauration et NativeResume,
pannes physiques, runtimes/worker Docker/Podman/Kubernetes réels, fournisseurs,
réseau métier, charge100 et suite complète restent hors preuve. Aucune durée
worker, fin métier ou libération de quota n'est inférée ; `billable=false`
reste inchangé. G09 demeure FAIL et 0/12 gates production sont clos.

La prochaine tranche doit établir le permis de reprise Unsubmitted
explicitement audité, lié au vrai original et sans réarmement d'attempt,
start, claim ou délai consommé. Les demandes centrales/Resume existants ne
constituent pas ce permis. La cause de pile précise reste un diagnostic
ouvert, distinct de cette convergence vérifiée. Aucun nouveau contrat
opérateur, service ou format d'architecture n'est introduit ; les manuels
EN/FR ne sont pas modifiés par ce lot de preuves.
