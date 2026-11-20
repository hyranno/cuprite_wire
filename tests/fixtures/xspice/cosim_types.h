/* The digital types cosim.h uses without defining them (from XSPICE's digital.h). */
typedef enum { ZERO, ONE, UNKNOWN } Digital_State_t;
typedef enum { STRONG, RESISTIVE, HI_IMPEDANCE, UNDETERMINED } Digital_Strength_t;
typedef struct {
    Digital_State_t state;
    Digital_Strength_t strength;
} Digital_t;

#include "ngspice/cosim.h"
