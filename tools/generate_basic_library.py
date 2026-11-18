#!/usr/bin/env python3
"""Generate the basic part library of cuprite_wire (`libs/`).

Writes `libs/symbols/cuprite_basic.kicad_sym` and the footprints in
`libs/footprints/cuprite_basic.pretty/`. The SPICE models referred to by the symbols
(`libs/symbols/cuprite_basic.sp`) are written by hand.

The library is made for this project and released under its license (MIT). Nothing is
copied from the KiCad libraries: pinouts follow the common industry pinouts of the
packages, and land patterns are simple pads sized from nominal package dimensions.

Usage: python3 tools/generate_basic_library.py
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "libs"
LIB = "cuprite_basic"
GENERATOR = "cuprite_wire_generate_basic_library"
FONT = "(effects (font (size 1.27 1.27)))"
HIDDEN = "(effects (font (size 1.27 1.27)) (hide yes))"


def q(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


# --- Symbols -------------------------------------------------------------------------------

def prop(name, value, at=(0, 0), hidden=True):
    return f"\t\t(property {q(name)} {q(value)} (at {at[0]} {at[1]} 0) {HIDDEN if hidden else FONT})"


def pin(kind, number, name, at, angle, length=2.54, hidden=False):
    hide = " (hide yes)" if hidden else ""
    return (f"\t\t\t(pin {kind} line (at {at[0]} {at[1]} {angle}) (length {length}){hide} "
            f"(name {q(name)} {FONT}) (number {q(number)} {FONT}))")


def rect(x1, y1, x2, y2):
    return (f"\t\t\t(rectangle (start {x1} {y1}) (end {x2} {y2}) "
            "(stroke (width 0.254) (type default)) (fill (type none)))")


def poly(*points, fill="none"):
    pts = " ".join(f"(xy {x} {y})" for x, y in points)
    return f"\t\t\t(polyline (pts {pts}) (stroke (width 0.254) (type default)) (fill (type {fill})))"


def symbol(name, reference, value, footprint, description, units, sim=(), pin_names=True):
    """`units`: {(unit, style): [lines]}."""
    out = [f"\t(symbol {q(name)}"]
    if not pin_names:
        out.append("\t\t(pin_names (offset 0) (hide yes))")
    out.append("\t\t(exclude_from_sim no) (in_bom yes) (on_board yes)")
    out.append(prop("Reference", reference, (2.54, 2.54), hidden=False))
    out.append(prop("Value", value, (2.54, -2.54), hidden=False))
    out.append(prop("Footprint", f"{LIB}:{footprint}"))
    out.append(prop("Datasheet", "~"))
    out.append(prop("Description", description))
    for key, text in sim:
        out.append(prop(key, text))
    for (unit, style), lines in units.items():
        out.append(f"\t\t(symbol {q(f'{name}_{unit}_{style}')}")
        out.extend(lines)
        out.append("\t\t)")
    out.append("\t)")
    return out


def two_pin(name, reference, footprint, description, body, sim=()):
    pins = [pin("passive", "1", "~", (0, 3.81), 270, 1.27), pin("passive", "2", "~", (0, -3.81), 90, 1.27)]
    return symbol(name, reference, reference, footprint, description,
                  {(0, 1): body, (1, 1): pins}, sim, pin_names=False)


def diode(name, footprint, description, params):
    body = [poly((-1.27, 1.27), (-1.27, -1.27)), poly((1.27, 1.27), (1.27, -1.27), (-1.27, 0), (1.27, 1.27))]
    pins = [pin("passive", "1", "K", (-3.81, 0), 0), pin("passive", "2", "A", (3.81, 0), 180)]
    sim = [("Sim.Device", "D"), ("Sim.Pins", "1=K 2=A"), ("Sim.Params", params)]
    return symbol(name, "D", name, footprint, description, {(0, 1): body, (1, 1): pins}, sim, pin_names=False)


def amp_body(x=0):
    return poly((x - 5.08, 5.08), (x + 5.08, 0), (x - 5.08, -5.08), (x - 5.08, 5.08), fill="background")


def amp_pins(out, inv, noninv):
    return [pin("input", noninv, "+", (-7.62, 2.54), 0), pin("input", inv, "-", (-7.62, -2.54), 0),
            pin("output", out, "~", (7.62, 0), 180)]


def power_pins(vminus, vplus):
    return [pin("power_in", vminus, "V-", (-2.54, -7.62), 90, 3.81),
            pin("power_in", vplus, "V+", (-2.54, 7.62), 270, 3.81)]


SP = "${KICAD9_SYMBOL_DIR}/cuprite_basic.sp"


def symbols():
    out = [f"(kicad_symbol_lib (version 20241209) (generator {q(GENERATOR)}) (generator_version \"9.0\")"]
    out += two_pin("R", "R", "R_0603", "Resistor (cuprite_wire basic library)", [rect(-1.016, -2.54, 1.016, 2.54)])
    out += two_pin("C", "C", "C_0603", "Capacitor (cuprite_wire basic library)",
                   [poly((-2.032, 0.762), (2.032, 0.762)), poly((-2.032, -0.762), (2.032, -0.762)),
                    poly((0, 2.54), (0, 0.762)), poly((0, -2.54), (0, -0.762))])
    out += two_pin("L", "L", "L_0603", "Inductor (cuprite_wire basic library)",
                   [rect(-0.762, -2.54, 0.762, 2.54)])
    out += diode("D", "D_SOD-123", "Small-signal diode, generic model (cuprite_wire basic library)",
                 "is=2.5n n=1.8 rs=0.6 cjo=2p")
    out += diode("LED", "LED_0603", "Light-emitting diode, generic red LED model (cuprite_wire basic library)",
                 "is=1e-19 n=1.9 rs=5 cjo=10p")
    out += symbol("OPAMP", "U", "OPAMP", "SOT-23-5",
                  "Single op-amp, SOT-23-5 pinout (cuprite_wire basic library)",
                  {(1, 1): [amp_body(), *amp_pins("1", "4", "3"), *power_pins("2", "5")]},
                  [("Sim.Device", "SUBCKT"), ("Sim.Library", SP), ("Sim.Name", "cuprite_opamp"),
                   ("Sim.Pins", "3=in+ 4=in- 5=vcc 2=vee 1=out")])
    out += symbol("OPAMP_DUAL", "U", "OPAMP_DUAL", "SOIC-8",
                  "Dual op-amp, SOIC-8 pinout (cuprite_wire basic library)",
                  {(1, 1): [amp_body(), *amp_pins("1", "2", "3")],
                   (2, 1): [amp_body(), *amp_pins("7", "6", "5")],
                   (3, 1): [rect(-5.08, -5.08, 0, 5.08), *power_pins("4", "8")]},
                  [("Sim.Device", "SUBCKT"), ("Sim.Library", SP), ("Sim.Name", "cuprite_opamp_dual"),
                   ("Sim.Pins", "1=out1 2=in1- 3=in1+ 4=vee 5=in2+ 6=in2- 7=out2 8=vcc")])
    out.append(")")
    return "\n".join(out) + "\n"


# --- Footprints ----------------------------------------------------------------------------

def footprint(name, description, pads, body, courtyard):
    """`pads`: [(number, x, y, w, h)]; `body` and `courtyard`: half sizes (x, y)."""
    out = [f"(footprint {q(name)}", "\t(version 20241229)", f"\t(generator {q(GENERATOR)})",
           '\t(layer "F.Cu")', f"\t(descr {q(description)})",
           f'\t(property "Reference" "REF**" (at 0 {-courtyard[1] - 1} 0) (layer "F.SilkS") '
           "(effects (font (size 1 1) (thickness 0.15))))",
           f'\t(property "Value" {q(name)} (at 0 {courtyard[1] + 1} 0) (layer "F.Fab") '
           "(effects (font (size 1 1) (thickness 0.15))))",
           "\t(attr smd)"]
    for layer, (x, y), width in (("F.Fab", body, 0.1), ("F.CrtYd", courtyard, 0.05)):
        out.append(f"\t(fp_rect (start {-x} {-y}) (end {x} {y}) (stroke (width {width}) (type solid)) "
                   f"(fill no) (layer {q(layer)}))")
    for number, x, y, w, h in pads:
        out.append(f"\t(pad {q(number)} smd roundrect (at {x} {y}) (size {w} {h}) "
                   '(layers "F.Cu" "F.Mask" "F.Paste") (roundrect_rratio 0.25))')
    out.append(")")
    return "\n".join(out) + "\n"


def chip(name, kind):
    # 1.6 x 0.8 mm chip (0603 inch / 1608 metric).
    return footprint(name, f"{kind}, 0603 (1608 metric) chip", [("1", -0.8, 0, 0.9, 1.0), ("2", 0.8, 0, 0.9, 1.0)],
                     (0.8, 0.4), (1.5, 0.75))


def footprints():
    out = {}
    for name, kind in (("R_0603", "Resistor"), ("C_0603", "Capacitor"), ("L_0603", "Inductor")):
        out[name] = chip(name, kind)
    # The cathode (pin 1) is on the left.
    out["LED_0603"] = chip("LED_0603", "LED (cathode: pad 1)")
    out["D_SOD-123"] = footprint("D_SOD-123", "Diode, SOD-123 (cathode: pad 1)",
                                 [("1", -1.65, 0, 0.9, 1.2), ("2", 1.65, 0, 0.9, 1.2)], (1.35, 0.8), (2.35, 1.0))
    # SOT-23-5: pins 1-3 down the left side, 4-5 up the right side, 0.95 mm pitch.
    out["SOT-23-5"] = footprint("SOT-23-5", "SOT-23, 5 pins, 0.95 mm pitch",
                                [("1", -1.15, -0.95, 1.0, 0.6), ("2", -1.15, 0, 1.0, 0.6), ("3", -1.15, 0.95, 1.0, 0.6),
                                 ("4", 1.15, 0.95, 1.0, 0.6), ("5", 1.15, -0.95, 1.0, 0.6)],
                                (0.8, 1.45), (1.9, 1.75))
    # SOIC-8, 3.9 mm body: pins 1-4 down the left side, 5-8 up the right side, 1.27 mm pitch.
    pads = [(str(i + 1), -2.6, -1.905 + 1.27 * i, 1.5, 0.6) for i in range(4)]
    pads += [(str(8 - i), 2.6, -1.905 + 1.27 * i, 1.5, 0.6) for i in range(4)]
    out["SOIC-8"] = footprint("SOIC-8", "SOIC, 8 pins, 3.9 mm body, 1.27 mm pitch", pads, (1.95, 2.45), (3.6, 2.7))
    return out


def main():
    (ROOT / "symbols").mkdir(parents=True, exist_ok=True)
    (ROOT / "symbols" / f"{LIB}.kicad_sym").write_text(symbols())
    pretty = ROOT / "footprints" / f"{LIB}.pretty"
    pretty.mkdir(parents=True, exist_ok=True)
    for name, text in footprints().items():
        (pretty / f"{name}.kicad_mod").write_text(text)


if __name__ == "__main__":
    main()
