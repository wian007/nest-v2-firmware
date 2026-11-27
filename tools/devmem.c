#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <stdint.h>
#include <unistd.h>
int main(int argc, char **argv){
  if(argc<2){fprintf(stderr,"usage: devmem addr [value]\n");return 1;}
  off_t addr=strtoull(argv[1],0,0);
  int fd=open("/dev/mem",O_RDWR|O_SYNC);
  if(fd<0){perror("open /dev/mem");return 1;}
  long ps=sysconf(_SC_PAGESIZE);
  off_t page=addr & ~((off_t)ps-1), off=addr-page;
  void *m=mmap(0,ps,PROT_READ|PROT_WRITE,MAP_SHARED,fd,page);
  if(m==MAP_FAILED){perror("mmap");return 1;}
  volatile uint32_t *p=(volatile uint32_t*)((char*)m+off);
  if(argc>=3){*p=(uint32_t)strtoull(argv[2],0,0);printf("0x%08x\n",*p);}
  else printf("0x%08x\n",*p);
  return 0;
}
