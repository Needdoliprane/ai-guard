# Installer depuis une release GitHub

[English](install.md) · [Métadonnées de release et mises à jour](README.fr.md) · [Index d'installation](../README.fr.md) · [Politique de sécurité](../../../SECURITY.fr.md)

Ce guide explique comment télécharger une release d'ai-guard, vérifier ce que vous avez reçu, l'installer à côté d'une version précédente et revenir en arrière. Remplacez `vX.Y.Z` par le tag choisi. Les archives sont construites par le [workflow de release](../../../.github/workflows/release.yaml) et publiées d'abord en brouillon ; un mainteneur le publie.

**État.** Rien de ceci n'a encore été exécuté sur une release publiée. La procédure décrit le workflow livré ; la première release en sera la première preuve. Une archive de release n'est pas un build qualifié pour la production : voir les [gates de préparation à la production](../../engineering/PRODUCTION_READINESS.md).

## Contenu d'une release

| Fichier | Contenu |
|---|---|
| `ai-guard-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | Binaires natifs (`bin/`, sans le générateur d'AC de démonstration, qui reste un outil local), `LICENSE`, `THIRD-PARTY-NOTICES`, `BUILD-INFO`. Construite sur Ubuntu 22.04. |
| `ai-guard-vX.Y.Z-aarch64-apple-darwin.tar.gz` | Idem pour macOS sur puce Apple. Construite sur macOS 15. |
| `ai-guard-vX.Y.Z-frontend.tar.gz` | Le bundle compilé du dashboard, avec la licence de Vite. |
| `ai-guard-vX.Y.Z-release-metadata.tar.gz` | `manifest/release-manifest.json`, `manifest/sbom.cdx.json`, le `manifest/SHA256SUMS` propre à l'outil, `BUILD-INFO`, `LICENSE`. |
| `ai-guard-vX.Y.Z-sbom.cdx.json` | Le même inventaire CycloneDX 1.6, isolé. |
| `SHA256SUMS` | SHA-256 de tous les fichiers ci-dessus. |

Les autres plateformes ne sont pas construites. Windows et Linux sur ARM n'ont pas d'archive.

## Télécharger et vérifier

```sh
TAG=vX.Y.Z
mkdir -p "$HOME/ai-guard-download/$TAG" && cd "$HOME/ai-guard-download/$TAG"
gh release download "$TAG" --repo Needdoliprane/ai-guard
```

Un brouillon n'est visible que des comptes disposant des droits d'écriture ; un téléchargement par navigateur convient aussi. Vérifiez ensuite chaque fichier conservé contre `SHA256SUMS` :

```sh
FILE=ai-guard-$TAG-x86_64-unknown-linux-gnu.tar.gz   # ou l'archive aarch64-apple-darwin
grep -F "  $FILE" SHA256SUMS | shasum -a 256 -c -      # Linux : sha256sum -c -
```

`shasum -a 256 -c SHA256SUMS` fonctionne aussi, mais signale comme manquants tous les fichiers non téléchargés.

### Ce que ces vérifications prouvent, et ce qu'elles ne prouvent pas

**Rien n'est signé cryptographiquement.** Il n'y a ni signature, ni attestation de build, ni provenance SLSA. `SHA256SUMS` est publié à côté des archives qu'il décrit : il détecte une corruption ou une confusion de fichiers, pas une page de release compromise, car qui peut modifier la release peut modifier les deux. Si ce risque compte pour vous, obtenez l'empreinte attendue de `SHA256SUMS` (ou de `release-manifest.json`) auprès d'un mainteneur par un canal de confiance distinct et comparez-la, ou construisez depuis les sources et comparez avec [`release verify`](README.fr.md#verify).

Chaque archive contient un fichier `BUILD-INFO`. Après extraction, vérifiez qu'il désigne le tag, le commit indiqué dans les notes de release et le run du workflow de release :

```sh
tar -xzf "$FILE"
cat "${FILE%.tar.gz}/BUILD-INFO"
```

Champs : `tag`, `commit`, `target` (archives natives seulement), `run`, `repository`. Les notes de release indiquent le même commit et le run CI qui a réussi dessus.

Vérifiez que le binaire annonce la même identité :

```sh
"${FILE%.tar.gz}/bin/agentsandbox" version --json
```

`build.revision` doit être égal à `commit`, et `version` au tag sans le `v`. C'est une identité déclarée, embarquée à la compilation, pas une attestation.

### Inventaire et notices

- `ai-guard-vX.Y.Z-sbom.cdx.json` est un inventaire CycloneDX 1.6 de ce que déclarent `Cargo.lock`, `front/package-lock.json` et les recettes d'images (cycle de vie `pre-build`). Ce n'est pas l'inventaire du binaire compilé.
- `THIRD-PARTY-NOTICES`, dans chaque archive native, liste les crates Rust liées aux binaires avec leurs textes de licence. Il couvre toutes les crates atteignables sur n'importe quelle plateforme et avec toutes les fonctionnalités, et peut donc en lister plus qu'un binaire n'en utilise. Il ne couvre ni la bibliothèque C de votre système ni la bibliothèque standard de Rust.
- Pour re-dériver le manifeste et le SBOM depuis les sources du tag, extrayez le tag, construisez le CLI et lancez `agentsandbox release verify --source . --manifest <dossier extrait>/manifest --source-only`. Un CLI modifié peut tout afficher : cette vérification vaut ce que vaut le CLI que vous avez construit.

## Notes par plateforme

**Linux x86_64.** Les binaires sont liés dynamiquement à la glibc et construits sur Ubuntu 22.04 ; ils exigent donc une glibc au moins aussi récente que celle de ce système (2.35). Ce plancher n'a pas été mesuré dans ce dépôt ; contrôlez votre système avec `ldd --version`, et l'exigence réelle d'un binaire avec `objdump -T bin/agentsandbox | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1`. Les distributions dont la glibc est plus ancienne doivent construire depuis les sources.

**macOS (puce Apple).** Les binaires ne sont ni signés ni notarisés. Un fichier téléchargé avec un navigateur porte un indicateur de quarantaine, et Gatekeeper refusera de l'exécuter. Après avoir vérifié l'empreinte, retirez l'indicateur du dossier extrait :

```sh
xattr -dr com.apple.quarantine "ai-guard-$TAG-aarch64-apple-darwin"
```

Les fichiers récupérés avec `gh` ou `curl` ne portent normalement pas cet indicateur. Ne désactivez pas Gatekeeper globalement.

**Moteur de conteneurs.** Les archives ne contiennent que les programmes de contrôle. Les images de contrôle et de worker sont construites par vous avec `agentsandbox build` ; voir le [guide local](../local/README.fr.md).

## Installer en conservant la version précédente

Extrayez chaque version dans son propre dossier et faites pointer un lien vers celle en service : revenir en arrière revient alors à changer le lien.

```sh
PREFIX=/opt/ai-guard
RELEASE="$PREFIX/${FILE%.tar.gz}"
sudo install -d "$PREFIX" &&
  sudo mkdir "$RELEASE" &&
  sudo tar -xzf "$FILE" -C "$RELEASE" --strip-components 1 &&
  "$RELEASE/bin/agentsandbox" version --json
```

Après vérification de la version, activez la release avec la commande suivante. Elle prépare le lien sous le même préfixe d’installation et remplace `current` en un seul renommage. Exécutez une seule installation ou activation à la fois. Si le dossier de release existe déjà, choisissez une nouvelle destination et inspectez le dossier existant avant de réessayer.

```sh
sudo sh -eu -c '
  prefix=$1
  release=$2
  current="$prefix/current"
  test -x "$release/bin/agentsandbox"
  test ! -e "$current" || test -L "$current"
  swap=$(mktemp -d "$prefix/.current.XXXXXX")
  ln -s "$release" "$swap/link"
  case "$(uname -s)" in
    Darwin) mv -fh "$swap/link" "$current" ;;
    Linux)  mv -fT "$swap/link" "$current" ;;
    *)      exit 1 ;;
  esac
  rmdir "$swap"
' sh "$PREFIX" "$RELEASE" &&
  "$PREFIX/current/bin/agentsandbox" version --json
```

Pour une installation utilisateur, choisissez un dossier qui vous appartient et omettez `sudo`. Aucun service ni outil de mise à jour n’est installé.

## Mettre à jour

1. Lisez les notes de release et [Version du produit, identité de build et mises à jour manuelles](README.fr.md#versions-and-updates).
2. Arrêtez les services et redémarrages automatiques qui utilisent les données, puis faites une sauvegarde privée cohérente de tous les stockages, de la configuration, des références de credentials et des ancres d'audit, selon la [procédure de reprise entreprise](../enterprise/README.fr.md#exploiter-et-reprendre).
3. Installez la nouvelle version à côté de l'ancienne, vérifiez-la comme ci-dessus et changez le lien.
4. Reconstruisez vos images avec le nouveau CLI, appliquez-les par votre procédure de déploiement habituelle, puis vérifiez versions des composants, santé, refus et continuité de l'audit avant de rouvrir les admissions.

Les migrations sont des commandes explicites, une étape à la fois ; la version 0.0.1 n'en applique aucune automatiquement.

## Revenir en arrière

Reprenez la commande d’activation avec `RELEASE` réglé sur le dossier précédent, puis restaurez la sauvegarde faite avant la mise à jour dans une nouvelle destination. **Ne rétrogradez jamais une base déjà migrée**, ne modifiez pas son `user_version` et ne la rouvrez pas avec un binaire plus ancien : restaurez la sauvegarde cohérente. Sans sauvegarde, ne devinez pas ; conservez les données et passez par les canaux du [guide de contribution](../../../CONTRIBUTING.md).

## Signaler un problème de release

Pour une vulnérabilité suspectée, utilisez le canal privé de la [politique de sécurité](../../../SECURITY.fr.md). Pour le reste, ouvrez une issue en joignant `agentsandbox version --json` et le contenu de `BUILD-INFO`.
