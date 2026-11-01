# Revue indépendante ciblée — PKI locale de test

Revue source en lecture seule du 1 novembre 2026, checkout broker-service base 9b04e53. Aucun Cargo, test, OpenSSL, moteur, endpoint privé, certificat ou credential existant ouvert/exécuté par le reviewer. Les commandes consultées sont du code, pas des résultats d'exécution. Les nouvelles clés ne sont pas examinées. Le générateur reste un utilitaire local d'essai, sans qualification G11, NAS ou magasin global.

Gel relu : control_fs.rs 698bba002f09a3510a2cbfb4341f2326875791d0efed59db0eaea0042e4a3b77 ; ai-guard-demo-pki.rs a7da43dd251e9277ff6a6cec6ab8ac2682358642169feb7b99e0c4907c1a28ca ; tests/demo_pki.rs a385290a198be0056c191dd242ffc434b3a7f66cf1c6dd805652731d2ff234b8. SHA constatés par lecture/hachage.

## Constats initiaux traités à la lecture

- L'ancien contrôle du programme ne protégeait que le mode du fichier terminal. Un autre UID propriétaire, un parent modifiable ou son remplacement permettaient d'exécuter un programme différent avec accès aux clés nouvellement créées. TrustedExecutable (control_fs.rs:586–654) réutilise maintenant walk/open_file, contrôles de système de fichiers et ACL, exige propriétaire root/UID de contrôle, aucun bit d'écriture groupe/autres ni set-ID, conserve FD et metadata, revalide parent et état du fichier avant chaque spawn (binaire:359–367). Les courses d'un UID de contrôle/root déjà de confiance ne sont pas promises résolues.
- Les deux anciens tests TLS acceptaient toute erreur. Le test:167–203 exige maintenant NotValidForName ou UnknownIssuer dans la chaîne interne, écarte timeout/non-connexion et réaffirme un succès de l'endpoint entre/après les négatifs. Aucun diagnostic brut n'est imprimé. Le nom interne Debug est un couplage de harnais au Rustls verrouillé ; un changement provoquera un FAIL, pas un PASS réseau.
- L'ancien négatif -attime acceptait n'importe quel exit non nul. Le test:104–112 exige maintenant le diagnostic spécifique d'expiration, avec délai dix secondes et vérification de taille. Une option non prise en charge ne satisfait plus l'oracle. La taille est vérifiée après capture ; ce n'est pas un lecteur physiquement plafonné, mais l'entrée est ici un certificat neuf borné et l'outil de confiance.

Les trois régressions source test:230–301 vérifient remplacement conservant le FD, lien, modes dangereux/non-exécutable, parent 0777 refusé avant création de sortie, SAN malformés et conservation d'une destination existante. Elles n'exécutent pas le faux programme.

## Point de harnais encore ouvert au gel

Le serveur test:121–131 reçoit seulement `-cert server-chain.pem`. Dans le s_server LibreSSL public, ce paramètre passe par load_cert, qui ne lit qu'un X509 PEM, puis set_cert_key_stuff. L'intermédiaire concaténé n'est pas une garantie de chaîne servie. Sur cette implémentation, sans magasin d'intermédiaire explicite, le positif avec seulement la racine côté client peut échouer UnknownIssuer. Fournir la chaîne via l'option compatible réellement retenue, par exemple CAfile pour la construction côté serveur, ou consigner une preuve versionnée contraire avant de qualifier ce harnais. Cela annonce un FAIL explicite possible, pas un faux PASS ni une faille TLS du produit. Sources primaires consultées : https://raw.githubusercontent.com/libressl/openbsd/master/src/usr.bin/openssl/s_server.c (1051–1053,1308) et https://raw.githubusercontent.com/libressl/openbsd/master/src/usr.bin/openssl/apps.c (493–516). Aucun comportement de l'exécutable hôte n'a été inféré comme observé.

## Autres propriétés et limites

Sortie neuve exclusive, parents privés contrôlés, fichiers 0600 et dossier 0700, noms fixes, SAN ASCII bornés sans injection, RSA2048/SHA256, contraintes CA/pathlen et EKU serveur cohérentes. Numéros UUID aléatoires de 128 bits ; durée serveur 1–30 jours sous intermédiaire60/racine90. argv séparés, environnement vidé, sorties OpenSSL supprimées, stdin fermé, six étapes sous délais coopératifs 30/120 secondes avec kill/reap. Les appels FS synchrones ne sont pas une borne dure de temps global. Pas de suppression de l'existant, pas de trust/DNS automatique. Une erreur tardive peut laisser READY ; la documentation exige aussi le succès terminal de la commande.

Autodiscovery Cargo du binaire et tests Unix cohérents à la lecture ; compilation et TLS réels NOT_RUN par le reviewer. Le serveur loopback éphémère n'est qu'un harnais, avec fermeture explicite et kill_on_drop. Aucun renouvellement/révocation/PKI production, ni migration implicite du profil Keycloak HTTP vers HTTPS.

Avis : les trois corrections annoncées sont traitées dans le code relu ; seul le point de chaîne du serveur de test reste à résoudre/borner avant la première preuve GREEN. Aucune exigence de refonte crypto ou de campagne moteur supplémentaire.

## Addendum — correction du dernier point de chaîne

Gel suivant relu le 1 novembre 2026 : tests/demo_pki.rs 04dcd34978ebfb629cb5d9fe499e52cb500d79f67ccc450ae8f0936e615b4b30. Le seul ajout d'argv `-CAfile intermediate-ca.pem` donne désormais l'intermédiaire au constructeur de chaîne du serveur LibreSSL. La racine publique reste la seule autorité ajoutée au client Rust ; les deux négatifs causaux et les positifs avant/après sont conservés. Aucun changement produit, magasin système ou exigence de client-certificat induit. Le point ouvert précédent est traité à la lecture. GO de revue source pour les validations ciblées prévues ; aucun PASS d'exécution n'en est déduit, Green réel en cours chez Luna au moment de cette note.
