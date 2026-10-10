# Entry point for `nix-build packaging` from a checkout.
{ pkgs ? import <nixpkgs> { } }:

pkgs.callPackage ./whykey.nix { }
