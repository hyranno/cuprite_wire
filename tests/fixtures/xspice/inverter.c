/* A d_cosim model with one input and one output: the output is the inverted input
 * (unknown stays unknown). Built by the tests of src/spice/xspice.rs. */
#include "cosim_types.h"

static Digital_t input;
static int changed;

static void in_fn(struct co_info *info, unsigned int bit, Digital_t *value) {
    (void)info;
    (void)bit;
    input = *value;
    changed = 1;
}

static void step(struct co_info *info) {
    if (!changed)
        return;
    Digital_t output = {UNKNOWN, STRONG};
    if (input.state == ZERO)
        output.state = ONE;
    else if (input.state == ONE)
        output.state = ZERO;
    info->out_fn(info, 0, &output);
    changed = 0;
}

void Cosim_setup(struct co_info *info) {
    info->in_count = 1;
    info->out_count = 1;
    info->inout_count = 0;
    info->step = step;
    info->in_fn = in_fn;
    info->cleanup = 0;
    info->method = After_input;
}
