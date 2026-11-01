# PKI — vérification portable et documents avant commit

1 novembre 2026, revue indépendante en lecture seule. Aucun Cargo/test/OpenSSL/moteur, aucune fixture générée ou clé ouvert/exécuté par le reviewer. Complément à /tmp/ai-guard-demo-pki-final-evidence-review.md ; les résultats historiques ne sont pas rejoués ni réécrits.

**PASS vérification de l'archive portable** /private/tmp/ai-guard-pki-root.HCZMFu/portable-source.tar.gz, SHA constaté02a13540baa87c0a32a5ffb3e014198a31bfaebbf3be8f3d083d02a00817f56b. Inventaire logique puis en-têtes TAR bruts examinés indépendamment :163 entrées uniques, exclusivement régulières, noms relatifs sans traversée et exactement identiques au manifeste post-tests ;163/163 contenus SHA256 concordants. Aucune entrée AppleDouble, clé/certificat généré, lien, en-tête d'extension PAX/GNU, sidecar ACL/xattr/flags ou record final non nul. Les champs standards TAR de fichier ne sont pas assimilés à des métadonnées supplémentaires. Aucun contenu des AppleDouble originaux lu.

Originaux privés re-hachés inchangés : avant12681f32bc94a9f4d2b48d169ede1d482feeccff190d9527fd945ee53f9cb9c2 ; aprèsff292813c42fe5a560b546d8512ff04ac44fe29b309a416996665dbdd96e79b7. Ils conservent chacun163 membres source+163 AppleDouble ; la copie portable est un nouvel artefact vérifié, pas une correction rétroactive des originaux. Aucun test supplémentaire nécessaire pour cet assainissement.

**Revue documentaire ciblée favorable**, avec précisions mineures transmises à Root, propriétaire des corrections. Diff des guides quickstart/sso et de CURRENT_STATE, DECISIONS, IMPLEMENTATION_PLAN, INFRASTRUCTURE_PKI, PRODUCTION_READINESS, VALIDATION relu, ainsi que guide demo-pki et nouveau README de preuve. Cohérents sur trois fonctions uniques, quatre FAIL distincts conservés, test réellement --offline sans --locked, unique arête du lock, différence entre contenus manifestés et inventaire AppleDouble, absence de confiance/DNS automatique. G11 reste BLOCKED, G09 FAIL, D5 ouvert,0/12 gates clos ; aucun nouveau runtime, HTTPS Keycloak/LLM, PKI production ou build release qualifié.

Précisions rédactionnelles avant clôture :

- VALIDATION, paragraphe PASS du1er novembre : RC2 et diagnostic stdout appartiennent au diagnostic direct distinct. Le GREEN final prouve un refus causal non nul avec motif dans l'un des deux flux, sans enregistrer le RC/flux interne exact. Attribuer explicitement ces détails au diagnostic.
- README, ligne tableau expiration «deux flux bornés» : préférer «taille des deux flux vérifiée après capture», conformément à la source et à la revue, pour ne pas promettre un collecteur physiquement plafonné.
- Remplacer/dater l'attente de vérification portable dans README et les mentions de préparation CURRENT_STATE/VALIDATION avec le résultat indépendant ci-dessus. Aucun état historique ou FAIL à convertir.

Ces précisions n'ouvrent aucun défaut source ou nouvelle campagne ; aucune autre contradiction étayée dans le delta relu. Les liens452/10docs et diff-check sont rapportés PASS par Root, non rejoués par le reviewer.
