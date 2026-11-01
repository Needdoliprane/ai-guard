# PKI locale — revue finale source et preuves

1 novembre 2026, broker-service. Lecture seule des sources, logs, RC, manifestes et membres source des archives désignées. Aucun test/Cargo/OpenSSL/moteur exécuté par le reviewer ; aucune fixture générée, work/ ou clé ouverte. Revue limitée au générateur local d'essai et à son harnais ; les avis source antérieurs restent applicables.

## Gel et intégrité

SHA constatés : control_fs.rs 698bba002f09a3510a2cbfb4341f2326875791d0efed59db0eaea0042e4a3b77 ; binaire a7da43dd251e9277ff6a6cec6ab8ac2682358642169feb7b99e0c4907c1a28ca ; test Rustls 201701ccad94e389cc96fbcd1776635db860d0960a229b44ea6b8a0eb5ca6134. Produit inchangé entre les derniers diagnostics et le GREEN.

Capture finale : /private/tmp/ai-guard-demo-pki-rustls-green-20261101T021116Z. Manifestes avant/après : 163 noms identiques ; seule différence Cargo.lock, dc513f61ee7fb60d6a37d936da5818af2d4101586410d1e89b10b53c830ec37f →94d1d5968189288e002b1fe6ebb32eb04c37804602336d3e6d9d42de006a4641. Diff du lock sauvegardé au checkout : seule arête directe agentsandbox→tokio-rustls, aucune entrée/version ajoutée. Cargo.toml et les162 autres fichiers sont identiques dans les manifestes. Bytes des163 membres source de chacune des deux archives vérifiés contre leur manifeste, sans extraction.

Les tar privés originaux contiennent aussi163 membres AppleDouble `._*` chacun, hors manifeste et non lus ; tar macOS les masquait dans son inventaire ordinaire. Ce ne sont donc pas des archives strictement « source-only ». Constat accepté par Root : originaux privés conservés, copie portable sans métadonnées en préparation. Aucun membre de certificat/clé générée parmi les163 sources ; le contenu des sidecars n'a pas été audité. Vérification de la future copie assainie à faire séparément, sans répéter de test.

Manifestes SHA : avant00066a25b3b1d40844ace086f6ecbbdf543c907affbdcfff68b9ae575a6a051e ; après20c467f30834f36f82a7024f4e82c6796ac01014ea43786b27a299040cdd551f. Archives originales avant12681f32bc94a9f4d2b48d169ede1d482feeccff190d9527fd945ee53f9cb9c2 ; aprèsff292813c42fe5a560b546d8512ff04ac44fe29b309a416996665dbdd96e79b7.

## Résultats réellement lus

Commande enregistrée : `CARGO_INCREMENTAL=0 cargo test --offline --test demo_pki -- --test-threads=1`. Ce run n'utilisait pas --locked ; il a ajouté l'arête de lock attendue ci-dessus. cargo-demo-pki.rc=0 ; log :3 passed,0 failed/ignored/filtered,4,73s. Trois fonctions uniques : chaîne/TLS/expiration, refus des entrées et conservation de l'existant, TrustedExecutable/remplacement/liens/modes. Les répétitions historiques ne s'ajoutent pas à ce compte. fmt-check.rc=0, log vide ; clippy.rc=0, fin dev constatée1m40s, contrôle strict communiqué par Luna. Aucun build release ni qualification de performance déclaré par cette capture.

SHA logs finaux : test13b53f5535837d4435c8935610ea5722c2f7bc71dbe29d029e94be5f175ae406 ; clippy2c0d9ca7f73c3d0ba6870e5341f8a8002fe3e0327d671bd9a8777f7b15cda3d4. Les trois fichiers RC finaux contiennent0.

## Quatre FAIL conservés, distincts

1. Stub — /private/tmp/ai-guard-demo-pki-red-20261101-024154/01-demo-pki-red.log, RC101,0,24s : generation failed avant OpenSSL/TLS. SHA log2793329ec1b580f96753e24b938a83a0c476b6b5066ea5d0b9749e32fa9ddf30.
2. Parent0755 — capture green-20261101T014957Z, RC101,2PASS/1FAIL,0,53s. Diagnostic parent-diag-20261101T015518Z : parent755 sous umask022, CLI RC1 avec refus explicite du parent privé. SHA logae6468f8660abe37abe44d616deaae021dd1893240747dbf2f8f6c14baa1a3ea.
3. Expiration — green2-20261101T015635Z, RC101,2PASS/1FAIL,1,04s. expiry-diag-20261101T015753Z : génération RC0 ; verify -attime RC2, error10/certificate has expired sur stdout. Le harnais ne regardait que stderr. SHA log45752f7611671c6463b07133facb99dc1cd0583e1a4265850df9d3acb54c6d06.
4. Serveur — green3-20261101T015912Z, RC101,2PASS/1FAIL,2,31s. sserver-diag : RC1/getservbyname failure pour -accept host:port ; aucun échange TLS dans ce diagnostic. SHA log8ac20995bd7e5f30ffcfd8c267e04d23ce1670fe7a36829cc4d36cdc507d6270.

Ces diagnostics expliquent des gels précédents ; aucun FAIL n'est converti rétroactivement en PASS.

## Portée et conclusion

Le harnais final exerce une vraie génération OpenSSL et une vraie négociation Rustls sur listener FD127.0.0.1:0, chaîne leaf+intermédiaire, client approuvant seulement la nouvelle racine. Mauvais nom/AC inconnue exigent causes précises et positifs avant/après ; expiration exige son diagnostic dans l'un des deux flux ; erreurs transport et option inconnue ne valent pas refus certificat. No-overwrite et primitives de confiance sont vérifiés sans exécuter le faux programme. Aucun root/intermediate key dans le serveur de test ; abort+await au retour normal/Err ; échanges bornés. La taille des diagnostics d'expiration est vérifiée après capture, pas physiquement plafonnée en lecture.

Aucun nouveau défaut bloquant produit/source repéré. Preuve locale de trois fonctions au gel désigné et fmt/Clippy PASS ; archives originales à garder privées avec leur limite AppleDouble. Renouvellement/révocation, mTLS, NAS, trust global, G11/PKI production, HTTPS Keycloak/LLM et connexion produit intégrée restent hors qualification. Aucun gain, disponibilité de service ou garantie générale de latence déduit.
