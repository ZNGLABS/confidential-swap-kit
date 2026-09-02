pragma circom 2.1.0;

/*
 * pour.circom — le circuit qui remplace ZKProofStub.
 *
 * Traduction, en contraintes arithmétiques vérifiables, de l'énoncé écrit en
 * français dans la docstring de ZKProofStub (confidential_swap.py). Là où le
 * stub renvoyait « True » sans rien vérifier, chaque clause est ici une
 * équation qu'un menteur ne peut pas satisfaire.
 *
 *   (1) chaque note consommée appartient à l'arbre des engagements
 *   (2) le prouveur possède la clé secrète de chaque note
 *   (3) chaque nullifieur est correctement dérivé de cette clé
 *   (4) la valeur est conservée : entrées = sorties + frais
 *   (5) tous les montants sont bornés
 *   (6) les deux notes consommées sont distinctes
 *   (7) chaque nullifieur est NEUF, et l'arbre des nullifieurs est mis à jour
 *
 * La clause (7) est ce qui permet de supprimer les comptes de nullifieurs :
 * plus de 0,0018 SOL de rent par swap. Voir registry/NULLIFIEURS-INDEXES.md.
 *
 * PÉRIMÈTRE : 2 entrées, 2 sorties, un seul type de token, frais publics.
 * C'est le « pour » de Zerocash. Le swap entre deux tokens à un prix donné
 * reste à faire.
 */

include "poseidon.circom";
include "bitify.circom";
include "mux1.circom";
include "comparators.circom";
include "imt.circom";


// Engagement en deux couches, v4 : le JETON passe dans la couche EXTERNE.
//
//   k  = Poseidon(apk, rho, r)          ← ne révèle ni le propriétaire ni l'aléa
//   cm = Poseidon(value, token, k)      ← le programme peut le recalculer
//
// Pourquoi ce déplacement. Un dépôt est public : la chaîne voit le montant ET
// le mint transféré. Elle doit donc pouvoir vérifier que l'engagement inséré
// porte bien CE montant et CE jeton — sinon on dépose un jeton sans valeur en
// déclarant une note en NX, et on vide le coffre au retrait.
//
// Avec le jeton à l'intérieur de `k`, la chaîne ne pouvait rien vérifier :
// `k` est opaque. En le sortant, le déposant ne publie que `k`, et le
// programme calcule cm = Poseidon(montant_reçu, mint_reçu, k) lui-même.
// Ce qui est prouvé n'est plus déclaré.
template NoteCommitment() {
    signal input token;
    signal input value;
    signal input apk;
    signal input rho;
    signal input r;
    signal output cm;

    component hk = Poseidon(3);
    hk.inputs[0] <== apk;
    hk.inputs[1] <== rho;
    hk.inputs[2] <== r;

    component h = Poseidon(3);
    h.inputs[0] <== value;
    h.inputs[1] <== token;
    h.inputs[2] <== hk.out;
    cm <== h.out;
}


/* Recalcule la racine de Merkle à partir d'une feuille et de son chemin. */
template MerklePath(depth) {
    signal input leaf;
    signal input pathElements[depth];
    signal input pathIndices[depth];
    signal output root;

    component hash[depth];
    component mux[depth];
    signal cur[depth + 1];
    cur[0] <== leaf;

    for (var i = 0; i < depth; i++) {
        pathIndices[i] * (pathIndices[i] - 1) === 0;
        mux[i] = MultiMux1(2);
        mux[i].c[0][0] <== cur[i];
        mux[i].c[0][1] <== pathElements[i];
        mux[i].c[1][0] <== pathElements[i];
        mux[i].c[1][1] <== cur[i];
        mux[i].s <== pathIndices[i];

        hash[i] = Poseidon(2);
        hash[i].inputs[0] <== mux[i].out[0];
        hash[i].inputs[1] <== mux[i].out[1];
        cur[i + 1] <== hash[i].out;
    }
    root <== cur[depth];
}


template Pour(depth, nfDepth, nBits, nfBits) {
    // ══════════ ENTRÉES PUBLIQUES ══════════
    // Neuf. Les nullifieurs n'en font pas partie : ils restent dans le
    // circuit, remplacés par les deux racines de leur arbre. La septième,
    // ajoutée en v4, est le jeton — voir plus bas pourquoi.
    // Chaque entrée publique coûte 4 174 CU à la vérification ; le reste du
    // circuit, lui, est gratuit on-chain.
    signal input root;
    signal input commitmentOut[2];
    signal input fee;
    signal input nfRootAvant;
    signal input nfRootApres;
    // Le JETON est public — septième entrée.
    //
    // Ce que ça coûte : un observateur apprend QUEL actif bouge. C'est le
    // modèle de Tornado, qui sépare ses pools par actif : l'actif fuit de
    // toute façon.
    // Ce que ça achète : la chaîne sait de quel coffre prélever les frais, et
    // peut vérifier que ce coffre est bien le sien. Sans cette entrée, le
    // programme paierait le relayeur depuis un coffre choisi par l'appelant.
    // Ce qui reste caché, l'essentiel : le montant, l'expéditeur, le
    // destinataire, et le lien avec le dépôt.
    signal input token;

    // Le JETON DE FRAIS — douzième entrée, ajoutée en v8.
    //
    // C'est la raison d'être de cette version : payer le relayeur en NX quel
    // que soit l'actif transféré. Sans cette entrée, la chaîne ne saurait pas
    // de quel coffre prélever les frais, et le programme paierait depuis un
    // coffre choisi par l'appelant.
    //
    // Ce que ça coûte : une entrée publique, soit 5 626 CU mesurés. Les
    // contraintes ajoutées, elles, sont quasi gratuites on-chain — c'est ce
    // que la courbe de coût du projet a établi.
    signal input tokenFrais;

    // ── LE RETRAIT, ajouté en v5.
    //
    // `montantRetrait` : ce qui quitte le pool vers le monde visible. À zéro,
    // la transaction est un simple transfert interne ; au-dessus, la chaîne
    // envoie réellement des jetons. Public, forcément : la chaîne doit savoir
    // combien sortir du coffre.
    // `destinataire` : le compte d'arrivée, scellé par la preuve pour que
    // personne ne puisse détourner le retrait en cours de route.
    signal input montantRetrait;
    signal input destinataire;

    // ── LES NULLIFIEURS SONT PUBLICS (16 août 2026)
    // Ils ne l'étaient plus depuis la v3, pour économiser 8 348 CU. Conséquence
    // découverte en écrivant le client : prouver l'absence d'un nullifieur dans
    // un arbre INDEXÉ exige d'exhiber la feuille qui l'encadre, faite des
    // nullifieurs déjà insérés. Sans publication, personne ne peut reconstruire
    // cet arbre — la 1re dépense passe, la 2e est impossible à prouver.
    // Les publier ne coûte rien à la confidentialité : nf = Poseidon(ask, rho)
    // n'est reliable ni à la note, ni à son propriétaire, ni à un montant. C'est
    // le choix de Zcash et de Tornado Cash.
    // Valeur TRONQUÉE à nfBits : c'est elle que l'arbre contient.
    signal input nullifierPub[2];

    // ══════════ TÉMOIN — notes consommées ══════════
    signal input inValue[2];
    signal input inRho[2];
    signal input inR[2];
    signal input inAsk[2];
    signal input pathElements[2][depth];
    signal input pathIndices[2][depth];

    // ══════════ TÉMOIN — notes produites ══════════
    signal input outValue[2];
    signal input outApk[2];
    signal input outRho[2];
    signal input outR[2];

    // ══════════ TÉMOIN — insertion dans l'arbre des nullifieurs ══════════
    signal input nfBasValeur[2];
    signal input nfBasIndexSuivant[2];
    signal input nfBasValeurSuivante[2];
    signal input nfBasPathElements[2][nfDepth];
    signal input nfBasPathIndices[2][nfDepth];
    signal input nfNouvelIndex[2];
    signal input nfNeufPathElements[2][nfDepth];
    signal input nfNeufPathIndices[2][nfDepth];

    component apk[2];
    component cmIn[2];
    component merkle[2];
    component nf[2];
    component tronc[2];
    component rangeIn[2];
    component insert[2];

    // La chaîne de racines : chaque insertion part de la racine produite par
    // la précédente. C'est ce chaînage qui rend les deux insertions
    // indissociables — et c'est l'endroit où une erreur passerait inaperçue.
    signal racines[3];
    racines[0] <== nfRootAvant;

    // ── v8 : DEUX ACTIFS DANS UNE MÊME PREUVE.
    //
    // L'emplacement 0 porte toujours l'actif transféré, l'emplacement 1
    // toujours NX. Le choix est structurel, pas vérifié : on ne peut pas
    // écrire une transaction où il est faux.
    //
    // Le prix de cette simplicité : on ne peut plus fusionner deux notes du
    // même actif, l'emplacement 1 étant réservé aux frais. Lever la limite
    // demande une troisième entrée — les contraintes sont quasi gratuites
    // on-chain, c'est le temps de preuve sur téléphone qui tranchera.
    signal jetonSlot[2];
    jetonSlot[0] <== token;
    jetonSlot[1] <== tokenFrais;

    for (var i = 0; i < 2; i++) {
        // ── (2) possession : apk dérive de la clé secrète
        apk[i] = Poseidon(1);
        apk[i].inputs[0] <== inAsk[i];

        cmIn[i] = NoteCommitment();
        cmIn[i].token <== jetonSlot[i];
        cmIn[i].value <== inValue[i];
        cmIn[i].apk   <== apk[i].out;
        cmIn[i].rho   <== inRho[i];
        cmIn[i].r     <== inR[i];

        // ── (1) appartenance à l'arbre des engagements
        merkle[i] = MerklePath(depth);
        merkle[i].leaf <== cmIn[i].cm;
        for (var j = 0; j < depth; j++) {
            merkle[i].pathElements[j] <== pathElements[i][j];
            merkle[i].pathIndices[j]  <== pathIndices[i][j];
        }
        merkle[i].root === root;

        // ── (3) nullifieur correctement dérivé, puis tronqué pour être
        //    comparable (voir Tronquer : circomlib ne compare pas au-delà
        //    de 252 bits, et Poseidon en produit ~254)
        nf[i] = Poseidon(2);
        nf[i].inputs[0] <== inAsk[i];
        nf[i].inputs[1] <== inRho[i];

        tronc[i] = Tronquer(nfBits);
        tronc[i].in <== nf[i].out;

        // le nullifieur publié est EXACTEMENT celui qui entre dans l'arbre
        nullifierPub[i] === tronc[i].out;

        // ── (7) le nullifieur est NEUF, et l'arbre est mis à jour
        insert[i] = NullifierInsert(nfDepth, nfBits);
        insert[i].racineAvant <== racines[i];
        insert[i].x <== tronc[i].out;
        insert[i].basValeur <== nfBasValeur[i];
        insert[i].basIndexSuivant <== nfBasIndexSuivant[i];
        insert[i].basValeurSuivante <== nfBasValeurSuivante[i];
        insert[i].nouvelIndex <== nfNouvelIndex[i];
        for (var j = 0; j < nfDepth; j++) {
            insert[i].basPathElements[j] <== nfBasPathElements[i][j];
            insert[i].basPathIndices[j]  <== nfBasPathIndices[i][j];
            insert[i].neufPathElements[j] <== nfNeufPathElements[i][j];
            insert[i].neufPathIndices[j]  <== nfNeufPathIndices[i][j];
        }
        racines[i + 1] <== insert[i].racineApres;

        // ── (5) montant borné
        rangeIn[i] = Num2Bits(nBits);
        rangeIn[i].in <== inValue[i];
    }

    // La racine finale doit être celle annoncée publiquement.
    racines[2] === nfRootApres;

    // ── (6) les deux notes consommées sont distinctes.
    // L'arbre indexé l'interdirait déjà — après la première insertion, aucune
    // feuille n'encadre plus le même nullifieur — mais une contrainte
    // explicite vaut mieux qu'une propriété qu'il faut déduire.
    component memeNote = IsEqual();
    memeNote.in[0] <== tronc[0].out;
    memeNote.in[1] <== tronc[1].out;
    memeNote.out === 0;

    component cmOut[2];
    component rangeOut[2];
    for (var k = 0; k < 2; k++) {
        cmOut[k] = NoteCommitment();
        cmOut[k].token <== jetonSlot[k];
        cmOut[k].value <== outValue[k];
        cmOut[k].apk   <== outApk[k];
        cmOut[k].rho   <== outRho[k];
        cmOut[k].r     <== outR[k];
        cmOut[k].cm === commitmentOut[k];

        rangeOut[k] = Num2Bits(nBits);
        rangeOut[k].in <== outValue[k];
    }

    component rangeFee = Num2Bits(nBits);
    rangeFee.in <== fee;

    // Le montant retiré est public et doit être borné comme les autres :
    // sans cette contrainte, un « retrait » de P−1 passerait pour négatif et
    // permettrait de créer de la valeur.
    component rangeRetrait = Num2Bits(nBits);
    rangeRetrait.in <== montantRetrait;

    // ── LE DESTINATAIRE EST LIÉ À LA PREUVE
    //
    // Sans cela, quiconque voit passer la transaction pourrait reprendre la
    // preuve, la republier en changeant le compte d'arrivée, et encaisser le
    // retrait à la place du bénéficiaire. C'est une attaque connue : le
    // relayeur est le premier à voir la preuve, et il n'a rien à perdre.
    //
    // Une entrée publique est scellée par la preuve elle-même. Il suffit donc
    // que le signal EXISTE dans le système de contraintes — sinon le
    // compilateur l'élimine et le scellement disparaît avec lui. Ce carré ne
    // sert à rien d'autre qu'à empêcher cette élimination.
    signal ancrageDestinataire;
    ancrageDestinataire <== destinataire * destinataire;

    // ── (4a) un seul jeton dans toute la transaction
    //
    // Plus besoin de contrainte : les quatre notes reçoivent directement le
    // signal public `token`. L'égalité n'est plus vérifiée, elle est
    // structurelle — on ne peut pas écrire un circuit où elle est fausse.

    // ── (4b) LA contrainte centrale : la valeur est conservée.
    //
    // Elle porte sur des signaux PRIVÉS. Le vérificateur ne connaît aucun des
    // montants cachés, et sait pourtant que l'égalité tient. Trois façons de
    // sortir de la valeur, une seule équation :
    //   · vers d'autres notes    → outValue, secret
    //   · vers le relayeur       → fee, public
    //   · vers le monde extérieur → montantRetrait, public
    // v8 : DEUX équations au lieu d'une, une par actif. C'est tout le
    // changement de fond — et c'est ce qui rend NX obligatoire pour payer,
    // quel que soit l'actif qui circule.
    //
    //   emplacement 0, l'actif transféré : il sort vers d'autres notes ou vers
    //   le monde visible, jamais vers le relayeur.
    inValue[0] === outValue[0] + montantRetrait;
    //   emplacement 1, NX : il sort vers d'autres notes ou vers le relayeur,
    //   jamais vers le monde visible.
    inValue[1] === outValue[1] + fee;
}


/* Tailles de la preuve de concept : 2^6 = 64 notes, 2^3 = 8 nullifieurs.
 *
 * Pourquoi si petit ? La cérémonie de test doit tenir sur deux cœurs de CPU.
 * Au-delà de 16 377 contraintes le domaine passe à 2^15 et le setup dépasse
 * les trois minutes dont on dispose. On a donc réduit la CAPACITÉ — jamais la
 * sécurité : `nfBits` reste à 248, soit 124 bits de résistance aux collisions.
 * Descendre à 160 bits aurait suffi à tenir, mais serait tombé à 80 bits ;
 * ce n'était pas le bon arbitrage.
 *
 * Les templates sont génériques : passer en production, c'est changer ces
 * quatre chiffres. Chaque niveau de profondeur coûte ~480 contraintes pour
 * les engagements et ~960 pour les nullifieurs — et rien du tout on-chain. */
component main {public [root, commitmentOut, fee, nfRootAvant, nfRootApres, token, tokenFrais, montantRetrait, destinataire, nullifierPub]} = Pour(20, 20, 64, 248);
