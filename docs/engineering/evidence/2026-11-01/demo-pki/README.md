# PKI locale de test — 1er novembre 2026

Base `9b04e53fc44475c38083c4f2832197fbf507337d`, worktree broker-service. Le [générateur Rust](../../../../demo-pki.md) crée une sortie neuve privée, une racine, un intermédiaire et une chaîne serveur. Aucune infrastructure NAS, installation de confiance ou modification DNS. Ce lot ne qualifie ni G11, ni une PKI de production, ni HTTPS Keycloak.

## Résultat final

**PASS logiciel et TLS local ciblé : trois fonctions uniques**, RC0,4,73s après compilation6,70s. Le [log](final/cargo-demo-pki.log) sélectionne effectivement les trois tests, sans ignore : génération réelle par OpenSSL/LibreSSL, permissions/type/liens, conservation de l'existant, refus de SAN invalides et de parent/programme inadmissibles, remplacement de programme détecté, expiration explicitement refusée et vraie négociation TLS loopback. Rustls sert la chaîne et la seule clé serveur neuve ; reqwest valide racine et nom. Mauvais nom et AC inconnue exigent leur cause TLS précise, avec positifs avant/après ; une panne réseau ne suffit pas.

Commandes constatées :

```sh
CARGO_INCREMENTAL=0 cargo test --offline --test demo_pki -- --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Le premier test est **`--offline`, pas `--locked`** : il aligne le lock sur la nouvelle dépendance de test directe `tokio-rustls=0.26.6`, déjà transitive de reqwest. Le [delta](final/Cargo.lock.diff) ajoute uniquement cette arête au package agentsandbox ; aucun paquet ni version ajouté ou modifié. Fmt et [Clippy strict](final/clippy.log) terminent RC0, Clippy1min40s. Aucun build release, frontend, fournisseur IA, moteur ou SSO exécuté pour ce lot. Le serveur TLS de test est une vraie socket `127.0.0.1:0`, pas un endpoint applicatif ai-guard.

La [revue finale indépendante](final-review.md) vérifie code, logs, RC et portée. Les [constats source initiaux](initial-review.md) sont corrigés : sélection de programme protégée et revalidée, négatifs causaux. La [relecture du harnais Rustls](rustls-review.md) ne remplace pas son exécution réelle ci-dessus.

## Échecs conservés et expliqués

| Gel | Résultat constaté | Diagnostic / correction |
|---|---|---|
| [Stub avant implémentation](red-stub/01-demo-pki-red.log) | RED attendu, RC101,0,24s, avant OpenSSL/TLS | Générateur absent ; code écrit ensuite. |
| [Premier contrôle](parent-failure/cargo-demo-pki.log) | Deux PASS, génération FAIL | [Reproduction CLI](parent-diagnostic/cli.stderr) : TempDir par défaut sous umask022 crée0755 ; le produit exige0700 et refuse avant création de sortie. Fixtures explicitement0700 et refus0755 conservé. |
| [Contrôle suivant](expiry-failure/cargo-demo-pki.log) | Deux PASS, oracle d'expiration FAIL | Génération directe RC0, [verify RC2](expiry-diagnostic/verify.rc), diagnostic d'expiration sur [stdout](expiry-diagnostic/verify.stdout), stderr vide. Le test examine maintenant les deux flux, taille vérifiée après capture, avec exit non nul et motif précis. |
| [Serveur LibreSSL](bind-failure/cargo-demo-pki.log) | Deux PASS, serveur TLS quitté avant connexion | [Diagnostic](bind-diagnostic/server.stderr) : `getservbyname failure` sur host:port ; aucun listener restant. Le harnais utilise désormais Rustls sur un FD loopback retenu. Aucune ouverture globale du port. |

Ces quatre FAIL ne sont pas rétroactivement des PASS. Les répétitions ne sont pas additionnées aux trois fonctions uniques finales. Aucun délai produit ni validation de certificat n'est assoupli pour les corriger.

## Sources et limites des archives

Les [manifestes avant](final/SHA256SUMS.before) et [après](final/SHA256SUMS.after-tests) couvrent163 fichiers sélectionnés : Cargo/toolchain, Rust/tests, quatre fixtures JSON publiques et guide PKI. Seul Cargo.lock change intentionnellement. Les trois sources finales : control_fs `698bba002f09a3510a2cbfb4341f2326875791d0efed59db0eaea0042e4a3b77`, binaire `a7da43dd251e9277ff6a6cec6ab8ac2682358642169feb7b99e0c4907c1a28ca`, test `201701ccad94e389cc96fbcd1776635db860d0960a229b44ea6b8a0eb5ca6134`. Manifeste après SHA `20c467f30834f36f82a7024f4e82c6796ac01014ea43786b27a299040cdd551f`.

Les archives originales restent privées, SHA avant `12681f32bc94a9f4d2b48d169ede1d482feeccff190d9527fd945ee53f9cb9c2`, après `ff292813c42fe5a560b546d8512ff04ac44fe29b309a416996665dbdd96e79b7`. Le reviewer confirme163/163 contenus source, mais constate aussi163 membres AppleDouble hors manifeste. Ce sont des métadonnées macOS supplémentaires ; ne pas présenter ces archives comme strictement limitées aux163 fichiers. Originaux et résultats de tests restent conservés.

Root reconstruit une copie portable depuis les163 fichiers courants concordants, avec `COPYFILE_DISABLE=1` et les options tar excluant métadonnées spécifiques/ACL/xattrs/flags. SHA `02a13540baa87c0a32a5ffb3e014198a31bfaebbf3be8f3d083d02a00817f56b` ; **[vérification indépendante PASS](portable-docs-review.md) le1er novembre** :163 membres réguliers uniques, noms et hashes exactement conformes, aucun AppleDouble, PAX global/par-membre ou lien. Archives, exécutables, fixtures générées et toutes clés restent **hors Git**. Seuls logs, manifestes et notes assainis sont partagés ici.

Le générateur réutilise les contrôles de namespace/ACL/FS du contrôle ; root et l'UID de contrôle demeurent de confiance. Il n'atteste pas toute la chaîne de dépendances de l'exécutable. Les accès positionnels aux sorties reposent sur une table constante de dix noms : la collecte des dix FD réussit entièrement ou retourne une erreur avant accès ; les positions configuration/chaîne restent un invariant à conserver lors d'une évolution.

Environnement : Rust/Cargo1.99.1, macOS27.2/Darwin27.2 arm64 ; OpenSSL système observé `LibreSSL3.3.6`. FS exact **NOT_RECORDED**. Linux/Podman/Kubernetes, renouvellement/révocation, confiance navigateur, mTLS et raccord aux services restent **NOT_RUN** pour ce lot. G09 reste FAIL, D5 ouvert,0/12 gates clos.
