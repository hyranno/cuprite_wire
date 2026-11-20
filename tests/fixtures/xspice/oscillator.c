/* A d_cosim model without inputs: its output toggles every half period, given in seconds as the
 * first simulation argument. Built by the tests of src/spice/xspice.rs. */
#include <stdlib.h>
#include "cosim_types.h"

static double half_period, next;
static int level, started;

static void in_fn(struct co_info *info, unsigned int bit, Digital_t *value) {
    (void)info;
    (void)bit;
    (void)value;
}

static void step(struct co_info *info) {
    if (!started) {
        Digital_t low = {ZERO, STRONG};
        info->out_fn(info, 0, &low);
        started = 1;
    }
    while (info->vtime >= next) {
        level = !level;
        Digital_t output = {level ? ONE : ZERO, STRONG};
        info->out_fn(info, 0, &output);
        next += half_period;
    }
}

void Cosim_setup(struct co_info *info) {
    half_period = info->sim_argc > 0 ? atof(info->sim_argv[0]) : 1e-6;
    next = half_period;
    level = 0;
    started = 0;
    info->in_count = 0;
    info->out_count = 1;
    info->inout_count = 0;
    info->step = step;
    info->in_fn = in_fn;
    info->cleanup = 0;
    info->method = Normal;
}
