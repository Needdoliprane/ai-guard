# Contribuer à ai-guard

Merci de l'intérêt que vous portez à la contribution à ai-guard !

[English version](CONTRIBUTING.md)

## Pour commencer

Veuillez lire le [contrat d'architecture](docs/engineering/ARCHITECTURE_CONTRACT.md) et les [règles de code](docs/engineering/CODING_RULES.md) avant de soumettre des changements.

## Flux de développement

1.  Forkez le dépôt.
2.  Créez une branche de fonctionnalité.
3.  Assurez-vous que vos changements respectent les [gates de préparation production](docs/engineering/PRODUCTION_READINESS.md) du projet.
4.  Exécutez les tests localement :
    *   Rust : `cargo test --locked`
    *   Frontend : exécutez `(cd front && npm ci && npm test)` depuis la racine du dépôt. Utilisez la version de Node.js épinglée dans le [workflow CI](.github/workflows/ci.yaml) pour correspondre à la CI.
5.  Soumettez une pull request.

## Sécurité de la chaîne d'approvisionnement

Nous prenons la sécurité de notre chaîne d'approvisionnement au sérieux :
*   Nous utilisons des versions épinglées pour les dépendances (toolchain Rust, version de Node).
*   Tout le code est relu selon les [règles de code](docs/engineering/CODING_RULES.md).
*   Nous utilisons des environnements conteneurisés pour les builds et les tests afin de garantir l'isolation et la reproductibilité.
*   Nous évitons autant que possible les binaires publics préconstruits, en préférant construire depuis les sources dans des environnements contrôlés.
*   Toute mise à jour de dépendance doit être examinée attentivement pour ses implications de sécurité.
*   Les images de base dans `images/*.Dockerfile` sont épinglées par digest d'index. Mettez à jour un tag et son digest ensemble.
*   Un changement de `Cargo.lock` qui introduit une expression de licence absente de `.github/third-party-license-expressions.txt` fait échouer la CI : examinez la licence et ajoutez-la dans le même changement.
*   Les releases livrent `SHA256SUMS`, un SBOM CycloneDX et `BUILD-INFO`, sans signature cryptographique ; voir [Installer depuis une GitHub Release](docs/setup/release/install.md).

## Signaler une vulnérabilité

N'ouvrez pas d'issue publique. Suivez [SECURITY.md](SECURITY.md) ([FR](SECURITY.fr.md)).
