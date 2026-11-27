/* raw physical-memory access via mmap(/dev/mem):
 *   memdump <hexbase> <hexlen>            -> raw bytes on stdout
 *   memdump <hexbase> <hexlen> <hexbyte>  -> fill region with byte
 * (stock busybox has no devmem; read()/write() on /dev/mem reject >high_memory,
 *  mmap does not and lands uncached since those pfns are outside stock's memmap) */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
int main(int argc,char**argv){
  if(argc<3){fprintf(stderr,"usage: memdump base len [fillbyte]\n");return 2;}
  unsigned long base=strtoul(argv[1],0,16), len=strtoul(argv[2],0,16);
  int fill=argc>3;
  int fd=open("/dev/mem",(fill?O_RDWR:O_RDONLY)|O_SYNC); if(fd<0){perror("open");return 1;}
  unsigned char*p=mmap(0,len,PROT_READ|(fill?PROT_WRITE:0),MAP_SHARED,fd,base);
  if(p==MAP_FAILED){perror("mmap");return 1;}
  if(fill){ memset(p,(int)strtoul(argv[3],0,16),len); msync(p,len,MS_SYNC); printf("filled 0x%lx+0x%lx with 0x%02lx\n",base,len,strtoul(argv[3],0,16)); return 0; }
  unsigned long off=0; while(off<len){ ssize_t n=write(1,p+off,len-off); if(n<=0){perror("write");return 1;} off+=n; }
  return 0;
}
