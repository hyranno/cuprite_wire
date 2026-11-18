* SPICE models of the cuprite_wire basic library (libs/symbols/cuprite_basic.kicad_sym).
* Written for cuprite_wire and released under its license (MIT).

* A single-pole op-amp: open-loop gain 100k, pole at 10 Hz (gain-bandwidth 1 MHz),
* output resistance 50 ohm. Supply rails, slew rate and offsets are not modeled;
* the supply pins are only loaded with 1 Gohm.
.subckt cuprite_opamp in+ in- vcc vee out
Rin in+ in- 10Meg
G1 0 n1 in+ in- 1m
R1 n1 0 100Meg
C1 n1 0 159p
E1 n2 0 n1 0 1
Rout n2 out 50
Rvcc vcc 0 1G
Rvee vee 0 1G
.ends cuprite_opamp

* Two cuprite_opamp in the SOIC-8 dual op-amp pinout.
.subckt cuprite_opamp_dual out1 in1- in1+ vee in2+ in2- out2 vcc
X1 in1+ in1- vcc vee out1 cuprite_opamp
X2 in2+ in2- vcc vee out2 cuprite_opamp
.ends cuprite_opamp_dual
