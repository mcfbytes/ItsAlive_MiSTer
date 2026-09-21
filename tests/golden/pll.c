/*
 * Golden-vector generator for the itsalive PLL solver.
 *
 * getPLLdiv(), findPLLpar() and setPLL() below are transcribed verbatim from
 * Main_MiSTer's video.cpp at upstream commit 6cda9cc -- lines 218-222, 224-260
 * and 262-318 respectively.  The only edits are:
 *
 *   - the printf() tracing and the PROFILE_FUNCTION() macro are gone;
 *   - vmode_custom_t is reduced to the union member setPLL() actually writes
 *     (item[] and Fpix), so no MiSTer headers are needed;
 *   - setPLL() copies its locals c, m and k into three globals at the end so
 *     main() can print them.  Upstream keeps them as locals and only the
 *     derived item[] values escape.
 *
 * Everything else, including the exact float literals, is the upstream text.
 * Those literals matter: 0.05f and 0.95f promote to double at the comparison,
 * and 0.05f as a double is 0.05000000074505805969..., not 0.05.  Normalising
 * them would change which C the search settles on for some pixel clocks, so
 * they are left as written.
 *
 * Run gen.sh to compile this and rewrite pll.json in place.  Nothing in the
 * Rust build compiles this file; the JSON is checked in and the Rust test only
 * reads it.
 */

#include <stdint.h>
#include <stdio.h>

typedef struct
{
	uint32_t item[32];
	double Fpix;
} vmode_custom_t;

/* video.cpp:218-222 */
static uint32_t getPLLdiv(uint32_t div)
{
	if (div & 1) return 0x20000 | (((div / 2) + 1) << 8) | (div / 2);
	return ((div / 2) << 8) | (div / 2);
}

/* video.cpp:224-260 */
static int findPLLpar(double Fout, uint32_t *pc, uint32_t *pm, double *pko)
{
	uint32_t c = 1;
	while ((Fout*c) < 400) c++;

	while (1)
	{
		double fvco = Fout*c;
		uint32_t m = (uint32_t)(fvco / 50);
		double ko = ((fvco / 50) - m);

		fvco = ko + m;
		fvco *= 50.f;

		if (ko && (ko <= 0.05f || ko >= 0.95f))
		{
			if (fvco > 1500.f)
			{
				return 0;
			}
			c++;
		}
		else
		{
			*pc = c;
			*pm = m;
			*pko = ko;
			return 1;
		}
	}

	//will never reach here
	return 0;
}

/* Added: setPLL()'s locals, so main() can print C, M and K. */
static uint32_t g_c, g_m, g_k;

/* video.cpp:262-318 */
static void setPLL(double Fout, vmode_custom_t *v)
{
	double Fpix;
	double fvco, ko;
	uint32_t m, c;

	if (!findPLLpar(Fout, &c, &m, &ko))
	{
		c = 1;
		while ((Fout*c) < 400) c++;

		fvco = Fout*c;
		m = (uint32_t)(fvco / 50);
		ko = ((fvco / 50) - m);

		//Make sure K is in allowed range.
		if (ko <= 0.05f)
		{
			ko = 0;
		}
		else if (ko >= 0.95f)
		{
			m++;
			ko = 0;
		}
	}

	uint32_t k = ko ? (uint32_t)(ko * 4294967296) : 1;

	fvco = ko + m;
	fvco *= 50.f;
	Fpix = fvco / c;

	v->item[9]  = 4;
	v->item[10] = getPLLdiv(m);
	v->item[11] = 3;
	v->item[12] = 0x10000;
	v->item[13] = 5;
	v->item[14] = getPLLdiv(c);
	v->item[15] = 9;
	v->item[16] = 2;
	v->item[17] = 8;
	v->item[18] = 7;
	v->item[19] = 7;
	v->item[20] = k;

	v->Fpix = Fpix;

	/* Added. */
	g_c = c;
	g_m = m;
	g_k = k;
}

/*
 * 74.25 and 25.175 are the two modes itsalive ships (vmodes[0] and vmodes[6],
 * video.cpp:127 and :133).  The rest are other pixel clocks from vmodes[] and
 * exist only to give the transcription more surface to be wrong on.
 */
static const double rates[] = { 74.25, 25.175, 65, 27, 108, 148.5 };
#define NRATES (sizeof(rates) / sizeof(rates[0]))

int main(void)
{
	printf("{\n");
	printf("  \"generator\": \"tests/golden/gen.sh\",\n");
	printf("  \"source\": \"Main_MiSTer video.cpp @ 6cda9cc: getPLLdiv :218-222, findPLLpar :224-260, setPLL :262-318\",\n");
	printf("  \"rows\": [\n");

	for (unsigned r = 0; r < NRATES; r++)
	{
		vmode_custom_t v = { { 0 }, 0.0 };
		setPLL(rates[r], &v);

		printf("    { \"fout\": %.17g, \"c\": %u, \"m\": %u, \"k\": %u, \"fpix\": %.17g, \"item\": [",
			rates[r], g_c, g_m, g_k, v.Fpix);
		for (int i = 9; i <= 20; i++) printf("%s%u", (i == 9) ? "" : ", ", v.item[i]);
		printf("] }%s\n", (r + 1 == NRATES) ? "" : ",");
	}

	printf("  ]\n");
	printf("}\n");
	return 0;
}
