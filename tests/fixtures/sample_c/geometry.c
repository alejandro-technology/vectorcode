#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct Point {
    int x;
    int y;
    char label[64];
};

struct Rectangle {
    struct Point origin;
    struct Point dimensions;
    double area;
};

struct Point* create_point(int x, int y, const char* label) {
    struct Point* p = (struct Point*)malloc(sizeof(struct Point));
    if (p == NULL) {
        return NULL;
    }
    p->x = x;
    p->y = y;
    strncpy(p->label, label, sizeof(p->label) - 1);
    p->label[sizeof(p->label) - 1] = '\0';
    return p;
}

double calculate_area(struct Rectangle* rect) {
    int width = rect->dimensions.x;
    int height = rect->dimensions.y;
    rect->area = (double)(width * height);
    return rect->area;
}

void print_point(struct Point* p) {
    printf("Point(%d, %d) [%s]\n", p->x, p->y, p->label);
}

void free_point(struct Point* p) {
    if (p != NULL) {
        free(p);
    }
}
