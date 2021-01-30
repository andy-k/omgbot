ABOUT

OMGBot, Andy Kurnia's bot that can play OMGWords game on the Woogles.io
platform using Kurnia algorithm and data structures.


LICENSE

Copyright (C) 2020-2021 Andy Kurnia. All rights reserved.

This is NOT free software. Any contributions must be done only with the
understanding that Andy Kurnia has full rights to the entirety of the
repository.

This repository contains experimental stuffs. In the course of experimenting,
some ideas, code, or data from other sources may be used. Those may not come
with redistribution rights, and whoever interacts with this repository will
need to separately gain access to those.

The code and ideas in this repository have no warranties, they may be buggy.
Code taken from this repository should be visibly attributed to Andy Kurnia.

Ideas taken from this repository should be visibly attributed to Andy Kurnia.


SPECIAL LICENSE

Anyone acting for the Woogles.io platform is expressly permitted to use this
project to integrate the functionality therein with the Woogles.io platform.
This special license also covers the use of the other project that defines the
Kurnia algorithms and data structures used by this project; together, both
projects constitute the "upstream projects".

Notwithstanding any competing licenses, any changes or improvements or
adaptations to either project are to be made available for inclusion in the
upstream projects without requiring any changes to the licenses of the upstream
projects. For avoidance of doubt, all such contributed code are expressly not
GPL/AGPL (despite Woogles.io platform being (A)GPL) as they are a derivative
work of the upstream projects and not a derivative work of Woogles.io.


INITIAL SETUP

Put macondo.proto in src/. This file is in a different project.
https://github.com/domino14/macondo/blob/94ebd662d89f7b89d81ca7609705e8006fa54dc7/api/proto/macondo/macondo.proto

Generate *.kwg and *.klv files, put them in current directory when running.
(Refer to the wolges project.)

cargo run
