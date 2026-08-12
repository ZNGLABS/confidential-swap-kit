pragma circom 2.1.0;

/*
 * imt.circom — les briques de l'arbre de nullifieurs INDEXÉ.
 *
 * Conception validée en JavaScript avant d'écrire ce fichier :
 * voir registry/nullifier_tree.js (11 tests) et registry/NULLIFIEURS-INDEXES.md.
 *
 * Rappel du principe. Un arbre de Merkle ordinaire prouve l'appartenance, pas
 * l'absence. Ici chaque feuille porte un pointeur vers la valeur immédiatement
 * supérieure — l'ensemble forme une liste chaînée triée :
 *
 *     feuille = (valeur, indexSuivant, valeurSuivante)
 *
 * Prouver que x est absent revient à exhiber la feuille « basse » L telle que
 * L.valeur < x < L.valeurSuivante. Si x était déjà là, aucune feuille ne
 * satisfait cet encadrement.
 *
 * ⚠️ Le test JS l'a montré : l'encadrement seul ne suffit PAS — une feuille
 *    basse forgée le satisfait. Ce qui rend la preuve infalsifiable, c'est que
 *    cette feuille appartienne réellement à l'arbre. C'est ce que le gadget
 *    ci-dessous impose, et c'est le cœur de la sécurité.
 */

include "poseidon.circom";
include "bitify.circom";
include "comparators.circom";
include "mux1.circom";

/* Un emplacement vide vaut 0 ; une feuille occupée vaut le hachage de ses
 * trois champs. Distinguer les deux est nécessaire pour insérer. */
template LeafHash() {
    signal input valeur;
    signal input indexSuivant;
    signal input valeurSuivante;
    signal output out;

    component h = Poseidon(3);
    h.inputs[0] <== valeur;
    h.inputs[1] <== indexSuivant;
    h.inputs[2] <== valeurSuivante;
    out <== h.out;
}

/* Deux chaînes de hachage parallèles le long du MÊME chemin : l'une part de
 * l'ancienne feuille, l'autre de la nouvelle. On obtient d'un coup la racine
 * avant et la racine après, en garantissant que c'est bien le même
 * emplacement qui a changé — et lui seul. */
template MerkleUpdate(depth) {
    signal input ancienneFeuille;
    signal input nouvelleFeuille;
    signal input pathElements[depth];
    signal input pathIndices[depth];
    signal output ancienneRacine;
    signal output nouvelleRacine;

    component hA[depth];
    component hN[depth];
    component muxA[depth];
    component muxN[depth];
    signal curA[depth + 1];
    signal curN[depth + 1];

    curA[0] <== ancienneFeuille;
    curN[0] <== nouvelleFeuille;

    for (var i = 0; i < depth; i++) {
        pathIndices[i] * (pathIndices[i] - 1) === 0;

        muxA[i] = MultiMux1(2);
        muxA[i].c[0][0] <== curA[i];
        muxA[i].c[0][1] <== pathElements[i];
        muxA[i].c[1][0] <== pathElements[i];
        muxA[i].c[1][1] <== curA[i];
        muxA[i].s <== pathIndices[i];

        muxN[i] = MultiMux1(2);
        muxN[i].c[0][0] <== curN[i];
        muxN[i].c[0][1] <== pathElements[i];
        muxN[i].c[1][0] <== pathElements[i];
        muxN[i].c[1][1] <== curN[i];
        muxN[i].s <== pathIndices[i];

        hA[i] = Poseidon(2);
        hA[i].inputs[0] <== muxA[i].out[0];
        hA[i].inputs[1] <== muxA[i].out[1];
        curA[i + 1] <== hA[i].out;

        hN[i] = Poseidon(2);
        hN[i].inputs[0] <== muxN[i].out[0];
        hN[i].inputs[1] <== muxN[i].out[1];
        curN[i + 1] <== hN[i].out;
    }

    ancienneRacine <== curA[depth];
    nouvelleRacine <== curN[depth];
}

/* Réduit un élément de corps à ses NBITS bits de poids faible.
 *
 * Pourquoi : les nullifieurs sont des sorties de Poseidon, donc réparties sur
 * tout le corps (~254 bits). Or `LessThan` de circomlib exige des entrées qui
 * tiennent sur au plus 252 bits — comparer deux éléments quelconques n'est pas
 * sûr. On tronque donc le nullifieur à 248 bits : l'ordre devient bien défini,
 * et la résistance aux collisions reste de 124 bits, largement suffisante. */
template Tronquer(NBITS) {
    signal input in;
    signal output out;

    component bits = Num2Bits_strict();
    bits.in <== in;

    var acc = 0;
    for (var i = 0; i < NBITS; i++) {
        acc += bits.out[i] * (1 << i);
    }
    out <== acc;
}

/* Insère un nullifieur dans l'arbre indexé et renvoie la nouvelle racine.
 *
 * Les quatre clauses, telles que spécifiées dans NULLIFIEURS-INDEXES.md :
 *   1. la feuille basse appartient à l'arbre (racine avant) ;
 *   2. elle encadre bien x ;
 *   3. elle est mise à jour en (L.valeur, n, x) ;
 *   4. la nouvelle feuille (x, L.indexSuivant, L.valeurSuivante) est écrite
 *      à l'emplacement n, jusque-là vide.
 */
template NullifierInsert(depth, NBITS) {
    signal input racineAvant;
    signal input x;                    // déjà tronqué à NBITS bits

    // la feuille basse et son chemin
    signal input basValeur;
    signal input basIndexSuivant;
    signal input basValeurSuivante;
    signal input basPathElements[depth];
    signal input basPathIndices[depth];

    // l'emplacement libre où atterrit la nouvelle feuille
    signal input nouvelIndex;
    signal input neufPathElements[depth];
    signal input neufPathIndices[depth];

    signal output racineApres;

    // ── clause 2 : l'encadrement
    // L.valeur < x, toujours.
    component infA = LessThan(NBITS);
    infA.in[0] <== basValeur;
    infA.in[1] <== x;
    infA.out === 1;

    // x < L.valeurSuivante, SAUF si la feuille basse est le maximum courant
    // (valeurSuivante == 0), auquel cas il n'y a pas de borne haute.
    component estMax = IsZero();
    estMax.in <== basValeurSuivante;

    component infB = LessThan(NBITS);
    infB.in[0] <== x;
    infB.in[1] <== basValeurSuivante;

    // (estMax == 1) OU (infB == 1)  ⇔  estMax + infB - estMax*infB == 1
    signal produit;
    produit <== estMax.out * infB.out;
    estMax.out + infB.out - produit === 1;

    // ── clauses 1 et 3 : la feuille basse appartient à l'arbre, et on la
    //    remplace par (L.valeur, nouvelIndex, x)
    component ancienneBasse = LeafHash();
    ancienneBasse.valeur <== basValeur;
    ancienneBasse.indexSuivant <== basIndexSuivant;
    ancienneBasse.valeurSuivante <== basValeurSuivante;

    component nouvelleBasse = LeafHash();
    nouvelleBasse.valeur <== basValeur;
    nouvelleBasse.indexSuivant <== nouvelIndex;
    nouvelleBasse.valeurSuivante <== x;

    component majBasse = MerkleUpdate(depth);
    majBasse.ancienneFeuille <== ancienneBasse.out;
    majBasse.nouvelleFeuille <== nouvelleBasse.out;
    for (var i = 0; i < depth; i++) {
        majBasse.pathElements[i] <== basPathElements[i];
        majBasse.pathIndices[i] <== basPathIndices[i];
    }
    majBasse.ancienneRacine === racineAvant;   // ← c'est ce qui rend la
                                               //   feuille basse infalsifiable

    // ── clause 4 : l'emplacement nouvelIndex était vide, il reçoit la
    //    nouvelle feuille qui reprend le chaînage de l'ancienne basse
    component nouvelleFeuille = LeafHash();
    nouvelleFeuille.valeur <== x;
    nouvelleFeuille.indexSuivant <== basIndexSuivant;
    nouvelleFeuille.valeurSuivante <== basValeurSuivante;

    component insertion = MerkleUpdate(depth);
    insertion.ancienneFeuille <== 0;           // emplacement vide
    insertion.nouvelleFeuille <== nouvelleFeuille.out;
    for (var i = 0; i < depth; i++) {
        insertion.pathElements[i] <== neufPathElements[i];
        insertion.pathIndices[i] <== neufPathIndices[i];
    }
    insertion.ancienneRacine === majBasse.nouvelleRacine;

    racineApres <== insertion.nouvelleRacine;
}
